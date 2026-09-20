use super::{OllamaClient, OllamaError};
use futures_util::StreamExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// 取消标志的共享句柄（UI 侧点「取消」即置位，拉取循环据此中断）。
/// 用 `CancelFlag::default()` 得到未置位的标志。
pub type CancelFlag = Arc<AtomicBool>;

fn is_cancelled(c: &CancelFlag) -> bool {
    c.load(Ordering::Relaxed)
}

/// 进度节流器（v1.0.9 修「模型库页拉取时卡死」）。
///
/// Ollama 的 /api/pull 对每个下载块都发一行进度（快连接下每秒数百上千行）。
/// 1.0.4 事件驱动桥接后每行都会唤醒主线程 → 主循环被进度事件灌满 → 界面卡死。
/// 在源头节流：首次、阶段切换（清单→下载→完成）、≥1% 跳变、≥250ms 间隔或终值才上报，
/// 事件量从 ~1000/s 降到 ≤5/s，UI 观感无损。
pub struct ProgressThrottle {
    last_pct: f64,
    last_ms: u64,
    started: bool,
}

impl ProgressThrottle {
    pub fn new() -> Self {
        Self { last_pct: -1000.0, last_ms: 0, started: false }
    }

    /// 判定本次进度是否应上报（now_ms 由调用方注入时钟，便于单测）。
    pub fn should_report(&mut self, pct: f64, now_ms: u64) -> bool {
        let phase_changed = (pct < 0.0) != (self.last_pct < 0.0);
        let jump = (pct - self.last_pct).abs() >= 1.0;
        let elapsed = now_ms.saturating_sub(self.last_ms) >= 250;
        let first = !self.started;
        let done = pct >= 100.0;
        if first || phase_changed || done || elapsed || jump {
            self.last_pct = pct;
            self.last_ms = now_ms;
            self.started = true;
            true
        } else {
            false
        }
    }
}

impl Default for ProgressThrottle {
    fn default() -> Self {
        Self::new()
    }
}

/// POST /api/pull — 拉取模型，逐行 NDJSON 回调整进度（经节流）。
///
/// `cancel` 置位后立即返回 `ErrorKind::Cancelled`；函数返回时响应流被丢弃，
/// Ollama 侧据此中止下载（不会留下半份模型）。
///
/// v3.6.2 自愈：Ollama 的 blob 下载是**分段并发写盘**的，一旦中途被中止
/// （关窗 / 断网 / 重启服务 / 进程被杀），`blobs/` 下会残留
/// `sha256-<digest>-partial` 与 `-partial-N` 空分片；下次拉取同一模型时
/// Ollama 撞上这些不一致状态，底层直接断开 → 流内只回 `error="EOF"`
/// （HTTP 仍是 200）→ 用户看到 `[Api] EOF`，且**每次失败位置都不同**
/// （取决于它先撞上哪个残片）。
///
/// 这里做一层自愈：首次遇到裸 EOF 时，清理残留分片后**自动重试一次**。
/// 用户无需手动干预，且残留不会再累积成「怎么拉都失败」的死局。
pub async fn pull_model<F>(
    c: &OllamaClient,
    name: &str,
    cancel: CancelFlag,
    mut on_progress: F,
) -> Result<(), OllamaError>
where
    F: FnMut(f64, u64, u64) + Send,
{
    // 首轮尝试
    match pull_once(c, name, &cancel, &mut on_progress).await {
        Ok(()) => return Ok(()),
        Err(e) => {
            // 只在「裸 EOF + 用户没取消」时自愈重试：
            // 取消是用户意图，不该被覆盖；其他错误（403/未找到）重试也无用。
            let cancellable_retry = is_bare_eof_kind(&e) && !is_cancelled(&cancel);
            if !cancellable_retry {
                return Err(e);
            }
            // 清理上次中断留下的残片（失败不阻断，可能只是无残片）
            let _ = super::clean_partial_blobs();
        }
    }
    // 重试前确认用户没在间隙里点取消
    if is_cancelled(&cancel) {
        return Err(OllamaError::cancelled());
    }
    pull_once(c, name, &cancel, &mut on_progress).await
}

/// 判断错误是否为「裸 EOF」——只有这类才值得清理残片后重试。
/// 与 `ollama::is_bare_eof`（内部判定）保持同一判据，这里面向已构造的错误对象。
fn is_bare_eof_kind(e: &OllamaError) -> bool {
    if e.kind != super::ErrorKind::Api {
        return false;
    }
    let m = e.message.trim();
    m.eq_ignore_ascii_case("eof")
        || m.eq_ignore_ascii_case("unexpected eof")
        || m.to_ascii_lowercase().ends_with(": eof")
}

/// 单次拉取（不含重试逻辑），供 `pull_model` 调用两次。
async fn pull_once<F>(
    c: &OllamaClient,
    name: &str,
    cancel: &CancelFlag,
    on_progress: &mut F,
) -> Result<(), OllamaError>
where
    F: FnMut(f64, u64, u64) + Send,
{
    // pull 可能持续数小时，使用无总超时的独立客户端，避免大模型在慢网络下被切断。
    let c = c.long_timeout_clone();
    let resp = c
        .http
        .post(c.url("/api/pull"))
        .json(&serde_json::json!({ "name": name, "stream": true }))
        .send()
        .await
        .map_err(|e| OllamaError::connect(format!("无法连接 Ollama（{}）", e)))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OllamaError::api_from_body(status, &text));
    }

    let mut throttle = ProgressThrottle::new();
    let t0 = Instant::now();
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        // 用户取消：丢弃响应流（Ollama 随之中止下载），以中性结果返回
        if is_cancelled(cancel) {
            return Err(OllamaError::cancelled());
        }
        // 流传输中断（连接重置、对端关闭、超时等）归为 Network，而非 Parse
        let chunk = chunk.map_err(|e| OllamaError::network(e.to_string()))?;
        buf.extend_from_slice(&chunk);
        // 以换行符切分 NDJSON
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if is_cancelled(cancel) {
                return Err(OllamaError::cancelled());
            }
            let v: serde_json::Value = serde_json::from_str(line)
                .map_err(|e| OllamaError::parse(format!("解析进度失败: {}", e)))?;
            if let Some(err) = v["error"].as_str() {
                // 流内错误：推送/拉取失败时 Ollama 常仍返回 200，
                // 真实原因（如注册中心 403）藏在这里，须走同一套翻译。
                return Err(OllamaError::api_from_stream_error(err));
            }
            let total = v["total"].as_u64().unwrap_or(0);
            let completed = v["completed"].as_u64().unwrap_or(0);
            if v["status"].as_str() == Some("success") {
                on_progress(100.0, total, completed);
            } else if total > 0 {
                let pct = (completed as f64 / total as f64) * 100.0;
                // 节流：进度行可能每秒上千条，全部上报会灌满主循环（v1.0.9）
                if throttle.should_report(pct, t0.elapsed().as_millis() as u64) {
                    on_progress(pct, total, completed);
                }
            } else {
                // 某些阶段（如 pulling manifest）没有 total，传递 -1 表示不确定
                if throttle.should_report(-1.0, t0.elapsed().as_millis() as u64) {
                    on_progress(-1.0, 0, 0);
                }
            }
        }
    }
    Ok(())
}

/// POST /api/push — 推送模型到注册中心，逐行 NDJSON 回调整进度。
/// 与拉取同理支持取消。
pub async fn push_model<F>(
    c: &OllamaClient,
    name: &str,
    cancel: CancelFlag,
    mut on_progress: F,
) -> Result<(), OllamaError>
where
    F: FnMut(f64, u64, u64) + Send,
{
    let c = c.long_timeout_clone();
    let resp = c
        .http
        .post(c.url("/api/push"))
        .json(&serde_json::json!({ "name": name, "stream": true }))
        .send()
        .await
        .map_err(|e| OllamaError::connect(format!("无法连接 Ollama（{}）", e)))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OllamaError::api_from_body(status, &text));
    }

    let mut throttle = ProgressThrottle::new();
    let t0 = Instant::now();
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        if is_cancelled(&cancel) {
            return Err(OllamaError::cancelled());
        }
        let chunk = chunk.map_err(|e| OllamaError::network(e.to_string()))?;
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if is_cancelled(&cancel) {
                return Err(OllamaError::cancelled());
            }
            let v: serde_json::Value = serde_json::from_str(line)
                .map_err(|e| OllamaError::parse(format!("解析进度失败: {}", e)))?;
            if let Some(err) = v["error"].as_str() {
                // 流内错误：推送/拉取失败时 Ollama 常仍返回 200，
                // 真实原因（如注册中心 403）藏在这里，须走同一套翻译。
                return Err(OllamaError::api_from_stream_error(err));
            }
            let total = v["total"].as_u64().unwrap_or(0);
            let completed = v["completed"].as_u64().unwrap_or(0);
            if v["status"].as_str() == Some("success") {
                on_progress(100.0, total, completed);
            } else if total > 0 {
                let pct = (completed as f64 / total as f64) * 100.0;
                if throttle.should_report(pct, t0.elapsed().as_millis() as u64) {
                    on_progress(pct, total, completed);
                }
            } else {
                if throttle.should_report(-1.0, t0.elapsed().as_millis() as u64) {
                    on_progress(-1.0, 0, 0);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ProgressThrottle;

    // ---------- 裸 EOF 判定（v3.6.2 自愈重试的触发条件）----------

    fn api_err(msg: &str) -> super::super::OllamaError {
        super::super::OllamaError::api(msg)
    }

    #[test]
    fn bare_eof_is_recognized() {
        // Ollama 底层断开时原样回的就是这三个字母
        assert!(super::is_bare_eof_kind(&api_err("EOF")));
        assert!(super::is_bare_eof_kind(&api_err("eof")));
        assert!(super::is_bare_eof_kind(&api_err("  EOF  ")));
        assert!(super::is_bare_eof_kind(&api_err("unexpected EOF")));
        assert!(super::is_bare_eof_kind(&api_err("unexpected eof")));
        // 带前缀的形态（上游包装）
        assert!(super::is_bare_eof_kind(&api_err("dial tcp: EOF")));
    }

    #[test]
    fn non_eof_errors_do_not_trigger_retry() {
        // 这些重试无用，不该触发清理+重试
        assert!(!super::is_bare_eof_kind(&api_err("model not found")));
        assert!(!super::is_bare_eof_kind(&api_err("403 Forbidden")));
        assert!(!super::is_bare_eof_kind(&api_err("")));
        // Network 类（连接超时等）也不在此列，避免掩盖真实网络问题
        assert!(!super::is_bare_eof_kind(&super::super::OllamaError::network("timed out")));
        // 非 Api 类型一律不触发
        assert!(!super::is_bare_eof_kind(&super::super::OllamaError::connect("EOF")));
    }

    #[test]
    fn cancelled_is_not_retried() {
        // 取消是用户意图，绝不能被自愈流程覆盖
        assert!(!super::is_bare_eof_kind(&super::super::OllamaError::cancelled()));
    }

    #[test]
    fn throttle_first_and_final_always_report() {
        let mut t = ProgressThrottle::new();
        assert!(t.should_report(-1.0, 0)); // 首次（清单阶段）
        assert!(t.should_report(0.4, 10)); // 阶段切换 -1 → 下载
        assert!(t.should_report(100.0, 20)); // 终值必报
    }

    #[test]
    fn throttle_suppresses_event_storm() {
        let mut t = ProgressThrottle::new();
        assert!(t.should_report(0.0, 0));
        // 模拟风暴：100ms 内 1000 条微步进（每条 0.0005%、间隔 <250ms、跳变 <1%）全部压制
        let mut reported = 0;
        for i in 1..=1000 {
            let pct = i as f64 * 0.0005;
            let now = i as u64 / 10;
            if t.should_report(pct, now) {
                reported += 1;
            }
        }
        assert_eq!(reported, 0, "风暴内不应有任何上报");
    }

    #[test]
    fn throttle_reports_on_1pct_jump_even_when_fresh() {
        let mut t = ProgressThrottle::new();
        assert!(t.should_report(10.0, 0));
        // 时间没到 250ms，但跳了 1% 以上 → 应报
        assert!(t.should_report(11.5, 100));
        assert!(!t.should_report(11.9, 150), "未到阈值与间隔应压制");
        assert!(t.should_report(11.95, 360), "间隔到 250ms 应报（哪怕步进小）");
    }

    #[test]
    fn throttle_reports_phase_change_back_to_manifest() {
        let mut t = ProgressThrottle::new();
        assert!(t.should_report(50.0, 0));
        assert!(t.should_report(51.0, 300));
        assert!(t.should_report(-1.0, 310), "下载→清单阶段切换应报");
    }
}
