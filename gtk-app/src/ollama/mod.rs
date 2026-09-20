pub mod catalog;
pub mod chat;
pub mod create;
pub mod hardware;
pub mod library;
pub mod models;
pub mod pull;

use serde::{Deserialize, Serialize};

/// 统一的 Ollama 错误类型，序列化后传给前端展示
#[derive(Debug, Serialize)]
pub struct OllamaError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, serde::Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ErrorKind {
    /// 无法连接 localhost:11434（Ollama 未运行）
    Connect,
    /// 网络流传输中断（连接重置、超时、对端关闭等）
    Network,
    /// Ollama 返回非 2xx
    Api,
    /// 响应解析失败
    Parse,
    /// 用户主动取消（拉取/推送中途中断）
    Cancelled,
}

impl std::fmt::Display for OllamaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{:?}] {}", self.kind, self.message)
    }
}

impl std::error::Error for OllamaError {}

impl OllamaError {
    pub fn connect(msg: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Connect, message: msg.into() }
    }
    pub fn api(msg: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Api, message: msg.into() }
    }
    pub fn parse(msg: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Parse, message: msg.into() }
    }
    pub fn network(msg: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Network, message: msg.into() }
    }
    /// 用户主动取消（不是故障，调用方据此显示中性提示而非错误框）
    pub fn cancelled() -> Self {
        Self { kind: ErrorKind::Cancelled, message: "已取消".to_string() }
    }

    /// 把 Ollama / registry 返回的非 2xx 响应体解析成可读中文提示。
    /// 特别处理注册中心常见错误码（如匿名推送被拒绝）。
    pub fn api_from_body(status: reqwest::StatusCode, body: &str) -> Self {
        if let Some(code) = extract_registry_code(body) {
            if let Some(hint) = registry_hint(&code) {
                return Self::api(hint);
            }
        }
        // 401/403 但识别不出具体错误码时，仍给一条可行动提示，
        // 不再把原始 JSON 抛给用户（用户看不懂 code，也拿不到下一步动作）。
        if registry_status_without_code(status) {
            return Self::api(LOGIN_HINT);
        }
        Self::api(format!("{}: {}", status, body))
    }

    /// 把**流内** error 字段翻译成可读提示。
    ///
    /// v3.5.1 关键修复：推送/拉取失败时 Ollama 通常仍返回 **HTTP 200**，
    /// 真实失败原因放在 NDJSON 流的 `error` 字段里，形如
    /// `403: {"errors":[{"code":"ANONYMOUS_ACCESS_DENIED",...}]}`。
    /// 旧实现只修了 HTTP 非 2xx 分支（`api_from_body`），该分支对推送**永不触发**，
    /// 于是用户看到的仍是原始 JSON。
    pub fn api_from_stream_error(raw: &str) -> Self {
        if let Some(code) = extract_registry_code(raw) {
            if let Some(hint) = registry_hint(&code) {
                return Self::api(hint);
            }
        }
        let low = raw.to_ascii_lowercase();
        if low.contains("anonymous access denied") || low.contains("unauthorized") {
            return Self::api(LOGIN_HINT);
        }
        // 「403/401 + registry errors 结构」才认定是鉴权问题，避免误伤普通报错
        if (raw.contains("403") || raw.contains("401")) && raw.contains("errors") {
            return Self::api(LOGIN_HINT);
        }
        // 裸 EOF（v3.6.2 关键修复，用户实测反复踩中）：
        // 下载中断后 blobs 目录会留下 `-partial` / `-partial-N` 残片，
        // Ollama 重试时读到这些不一致的分片状态 → 底层直接断开 → 流内只回 error="EOF"，
        // HTTP 层仍是 200。原文 "EOF" 对用户毫无信息量，必须翻译成可行动提示。
        if is_bare_eof(raw) {
            return Self::api(EO_OF_HINT);
        }
        Self::api(raw.to_string())
    }
}

/// 判定流内 error 是否为「裸 EOF」——即除 EOF 外没有任何有效信息。
///
/// Ollama 在底层连接被断开时会把 Go 的 `io.EOF` 原样塞进 error 字段
/// （就三个字母 `EOF`），偶尔带 `unexpected EOF` / `EOF: ...` 等变体。
/// 这类错误**必须单独识别**，否则会走 `Self::api(raw)` 原文透传，
/// 用户只看到 `[Api] EOF` 三个字母，完全无法判断下一步该做什么。
fn is_bare_eof(raw: &str) -> bool {
    let s = raw.trim().trim_end_matches('.').trim();
    // 纯 EOF（大小写不敏感）
    if s.eq_ignore_ascii_case("eof") {
        return true;
    }
    // `unexpected EOF` / `unexpected eof` 等变体
    let low = s.to_ascii_lowercase();
    if low == "unexpected eof" || low.ends_with(": eof") || low.ends_with(": unexpected eof") {
        return true;
    }
    false
}

/// 下载中断导致残留分片时的统一可行动提示（裸 EOF 专用）。
///
/// 三件事讲清楚：为什么失败（残留分片/连接被切）、模型没坏（已下的完好）、怎么办（一键修复）。
const EO_OF_HINT: &str =
    "下载中断：Ollama 在读写模型文件时连接被切断，本次未能完成。\n\
     常见原因是上一次下载被中止（关窗、断网、重启服务）后，模型目录里留下了未完成的分片文件，\n\
     本次拉取撞上这些残片后无法继续。\n\n\
     • 已下载完成的其他模型不受影响，无需担心。\n\
     • 在下方点「修复下载」清理残留分片后重试；若仍失败，通常是网络在下载中途断开，\n\
       可在网络稳定时重试，或改用较小的模型/量化版本。\n\
     • 排查命令：`journalctl -u ollama -n 50`（服务端原因）。";

/// 注册中心需要登录时的统一可行动提示（HTTP 体与流内错误共用）。
const LOGIN_HINT: &str =
    "注册中心拒绝了本次请求：推送模型需要先登录 Ollama 账户。\n请在终端运行 `ollama login`，或访问 ollama.com 登录后再试。";

/// 注册中心错误码 → 可行动中文提示。两种来源（HTTP 响应体 / 流内 error 字段）共用同一套映射。
fn registry_hint(code: &str) -> Option<&'static str> {
    match code {
        "ANONYMOUS_ACCESS_DENIED" => Some(LOGIN_HINT),
        "UNAUTHORIZED" => Some(LOGIN_HINT),
        "NAME_INVALID" => Some("模型名称不符合注册中心规范。推送时需带上你的账户命名空间，如 `你的用户名/模型名:标签`。"),
        "DENIED" => Some(LOGIN_HINT),
        _ => None,
    }
}

/// 从任意文本中识别注册中心错误码。
///
/// 兼容两种形态：
/// ① HTTP 非 2xx 的 JSON 体：`{"errors":[{"code":"ANONYMOUS_ACCESS_DENIED",...}]}`
/// ② Ollama 把上游错误原样塞进流内 error 字段，常带状态码前缀：
///    `403: {"errors":[{"code":"ANONYMOUS_ACCESS_DENIED",...}]}`
fn extract_registry_code(text: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(c) = v["errors"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|e| e["code"].as_str())
        {
            return Some(c.to_string());
        }
    }
    // 退化路径：文本含转义或前缀（流内 error 字段），按已知码做子串扫描
    for code in ["ANONYMOUS_ACCESS_DENIED", "UNAUTHORIZED", "NAME_INVALID", "DENIED"] {
        if text.contains(code) {
            return Some(code.to_string());
        }
    }
    None
}

/// 状态码本身即鉴权失败（识别不出具体码时的兜底判据）
fn registry_status_without_code(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN
}

/// 模型信息（/api/tags 返回）
#[derive(Debug, Serialize, Clone)]
pub struct ModelInfo {
    pub name: String,
    pub model: String,
    pub size: u64,
    pub size_human: String,
    pub parameter_size: String,
    pub modified_at: String,
    pub family: String,
    pub format: String,
    pub quant: String,
}

/// 已加载模型（/api/ps 返回）
#[derive(Debug, Serialize, Clone)]
pub struct LoadedModel {
    pub name: String,
    pub model: String,
    pub size_vram: u64,
    pub size_vram_human: String,
    pub total: u64,
    pub total_human: String,
    pub expires_at: String,
}

/// 模型详情（/api/show 返回）
#[derive(Debug, Serialize, Clone)]
pub struct ModelDetail {
    pub license: String,
    pub modelfile: String,
    pub parameters: String,
    pub template: String,
    pub system: String,
    pub family: String,
    pub parameter_size: String,
    pub quantization: String,
}

/// 聊天消息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// 生成参数（覆盖 Ollama 原生指令能力：基础采样参数 + 高级 options + keep_alive/think/format）
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct GenParams {
    pub system: Option<String>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    pub num_ctx: Option<u32>,
    pub num_predict: Option<u32>,
    pub seed: Option<u32>,
    pub repeat_penalty: Option<f32>,
    /// 停止词（可为多个）
    pub stop: Option<Vec<String>>,
    // ---- 高级采样 options ----
    pub min_p: Option<f32>,
    pub typical_p: Option<f32>,
    pub presence_penalty: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub penalize_newline: Option<bool>,
    pub numa: Option<bool>,
    pub num_gpu: Option<i32>,
    pub main_gpu: Option<i32>,
    pub num_batch: Option<i32>,
    pub draft_num_predict: Option<i32>,
    // ---- 顶层控制 ----
    /// keep_alive：模型驻留时长，如 "5m" / "10s" / "-1"(常驻) / "0"(用完即卸)。None = 服务端默认
    pub keep_alive: Option<String>,
    /// think：推理模型是否输出思考过程（true/false）
    pub think: Option<bool>,
    /// format：输出格式，"json" 或 JSON Schema 字符串
    pub format: Option<String>,
    /// tools：函数调用工具定义（Ollama /api/chat 的 tools 数组）。None = 不启用
    pub tools: Option<Vec<serde_json::Value>>,
}

impl GenParams {
    /// 顶层参数（Ollama 接受直接放在请求体里的字段）
    pub fn to_option(&self) -> serde_json::Value {
        let mut v = serde_json::Map::new();
        if let Some(s) = &self.system {
            v.insert("system".into(), serde_json::Value::String(s.clone()));
        }
        if let Some(t) = self.temperature {
            v.insert("temperature".into(), t.into());
        }
        if let Some(p) = self.top_p {
            v.insert("top_p".into(), p.into());
        }
        if let Some(k) = self.top_k {
            v.insert("top_k".into(), k.into());
        }
        if let Some(c) = self.num_ctx {
            v.insert("num_ctx".into(), c.into());
        }
        if let Some(p) = self.num_predict {
            v.insert("num_predict".into(), p.into());
        }
        if let Some(s) = self.seed {
            v.insert("seed".into(), s.into());
        }
        if let Some(r) = self.repeat_penalty {
            v.insert("repeat_penalty".into(), r.into());
        }
        if let Some(stop) = &self.stop {
            if !stop.is_empty() {
                v.insert("stop".into(), serde_json::Value::Array(stop.iter().map(|s| s.clone().into()).collect()));
            }
        }
        if let Some(ka) = &self.keep_alive {
            v.insert("keep_alive".into(), serde_json::Value::String(ka.clone()));
        }
        if let Some(th) = self.think {
            v.insert("think".into(), th.into());
        }
        if let Some(fmt) = &self.format {
            v.insert("format".into(), serde_json::Value::String(fmt.clone()));
        }
        // tools：Ollama 要求放在请求体顶层（与 system/think/format 同级），不是 options 子对象
        if let Some(tools) = &self.tools {
            if !tools.is_empty() {
                v.insert("tools".into(), serde_json::Value::Array(tools.clone()));
            }
        }
        serde_json::Value::Object(v)
    }

    /// 高级采样参数（Ollama 要求放在请求体的 `options` 子对象里）
    pub fn options_value(&self) -> Option<serde_json::Value> {
        let mut v = serde_json::Map::new();
        if let Some(x) = self.min_p {
            v.insert("min_p".into(), x.into());
        }
        if let Some(x) = self.typical_p {
            v.insert("typical_p".into(), x.into());
        }
        if let Some(x) = self.presence_penalty {
            v.insert("presence_penalty".into(), x.into());
        }
        if let Some(x) = self.frequency_penalty {
            v.insert("frequency_penalty".into(), x.into());
        }
        if let Some(x) = self.penalize_newline {
            v.insert("penalize_newline".into(), x.into());
        }
        if let Some(x) = self.numa {
            v.insert("numa".into(), x.into());
        }
        if let Some(x) = self.num_gpu {
            v.insert("num_gpu".into(), x.into());
        }
        if let Some(x) = self.main_gpu {
            v.insert("main_gpu".into(), x.into());
        }
        if let Some(x) = self.num_batch {
            v.insert("num_batch".into(), x.into());
        }
        if let Some(x) = self.draft_num_predict {
            v.insert("draft_num_predict".into(), x.into());
        }
        if v.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(v))
        }
    }
}

/// 人类可读的字节大小
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut val = bytes as f64;
    let mut idx = 0;
    while val >= 1024.0 && idx < UNITS.len() - 1 {
        val /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.2} {}", val, UNITS[idx])
    }
}

/// 一次生成的统计信息（取自 Ollama 流式响应最后一帧 `done=true`）。
/// 对应 `ollama run --verbose` 暴露的指标，用于界面如实展示速度与耗时。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GenStats {
    /// 总耗时（纳秒）
    pub total_duration_ns: u64,
    /// 提示词（输入）token 数
    pub prompt_eval_count: u64,
    /// 提示词处理耗时（纳秒）
    pub prompt_eval_duration_ns: u64,
    /// 生成（输出）token 数
    pub eval_count: u64,
    /// 生成耗时（纳秒）
    pub eval_duration_ns: u64,
    /// 函数调用结果（Ollama 在 done 帧的 message.tool_calls）。None = 本次未触发工具调用
    pub tool_calls: Option<serde_json::Value>,
}

impl GenStats {
    const NS_PER_SEC: f64 = 1_000_000_000.0;

    /// 生成速度（tokens/秒）。与官方口径一致：仅用生成阶段耗时，不含提示词处理。
    pub fn eval_tokens_per_sec(&self) -> f64 {
        if self.eval_duration_ns == 0 || self.eval_count == 0 {
            return 0.0;
        }
        self.eval_count as f64 / (self.eval_duration_ns as f64 / Self::NS_PER_SEC)
    }

    /// 提示词处理速度（tokens/秒）
    pub fn prompt_tokens_per_sec(&self) -> f64 {
        if self.prompt_eval_duration_ns == 0 || self.prompt_eval_count == 0 {
            return 0.0;
        }
        self.prompt_eval_count as f64 / (self.prompt_eval_duration_ns as f64 / Self::NS_PER_SEC)
    }

    /// 总耗时（秒）
    pub fn total_secs(&self) -> f64 {
        self.total_duration_ns as f64 / Self::NS_PER_SEC
    }

    /// 是否有任何可用统计（全零时界面不应显示统计行）
    pub fn is_empty(&self) -> bool {
        self.total_duration_ns == 0 && self.eval_count == 0 && self.prompt_eval_count == 0
    }

    /// 简要摘要，如 `128 tokens · 24.5 tokens/s · 5.2s；提示 42 tokens`
    pub fn summary(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut s = String::new();
        if self.eval_count > 0 {
            let tps = self.eval_tokens_per_sec();
            if tps > 0.0 {
                s.push_str(&format!(
                    "{} tokens · {:.1} tokens/s",
                    self.eval_count, tps
                ));
            } else {
                s.push_str(&format!("{} tokens", self.eval_count));
            }
        }
        if self.total_duration_ns > 0 {
            if !s.is_empty() {
                s.push_str(" · ");
            }
            s.push_str(&format!("{:.2}s", self.total_secs()));
        }
        if self.prompt_eval_count > 0 {
            if !s.is_empty() {
                s.push_str("；");
            }
            s.push_str(&format!("提示 {} tokens", self.prompt_eval_count));
        }
        s
    }

    /// 把工具调用整理成可读文本（函数名 + 参数），用于界面展示。
    /// 兼容 Ollama 两种形态：原生 `{function:{name,arguments}}` 与旧式 `{name,arguments}`。
    /// 无调用或格式不符时返回空串。
    pub fn tool_calls_text(&self) -> String {
        let Some(tc) = &self.tool_calls else { return String::new() };
        let serde_json::Value::Array(calls) = tc else { return String::new() };
        if calls.is_empty() {
            return String::new();
        }
        let mut out = String::from("🔧 函数调用：\n");
        for call in calls {
            let name = call
                .get("function")
                .and_then(|f| f.get("name"))
                .or_else(|| call.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("未知函数");
            let args = call
                .get("function")
                .and_then(|f| f.get("arguments"))
                .or_else(|| call.get("arguments"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let pretty = serde_json::to_string_pretty(&args).unwrap_or_else(|_| args.to_string());
            out.push_str(&format!("• 函数：{name}\n  参数：\n{pretty}\n"));
        }
        out
    }
}

/// 从 Ollama 流式响应的某一帧 JSON 中提取统计字段
pub(crate) fn extract_stats(v: &serde_json::Value) -> GenStats {
    fn u64_of(v: &serde_json::Value, k: &str) -> u64 {
        // 不同 Ollama 版本可能把时长作为浮点返回，统一容错取 u64
        v[k].as_u64().or_else(|| v[k].as_f64().map(|f| f as u64)).unwrap_or(0)
    }
    let tool_calls = match &v["message"]["tool_calls"] {
        // Ollama 在 done 帧的 message.tool_calls 是函数调用数组；没有时保持 None
        serde_json::Value::Array(arr) if !arr.is_empty() => Some(serde_json::Value::Array(arr.clone())),
        _ => None,
    };
    GenStats {
        total_duration_ns: u64_of(v, "total_duration"),
        prompt_eval_count: u64_of(v, "prompt_eval_count"),
        prompt_eval_duration_ns: u64_of(v, "prompt_eval_duration"),
        eval_count: u64_of(v, "eval_count"),
        eval_duration_ns: u64_of(v, "eval_duration"),
        tool_calls,
    }
}

/// 增量 NDJSON 读取器：逐块喂入网络字节流，产出完整的 JSON 对象。
///
/// 存在意义：
/// 1. 可测试 —— 流式解析是全应用最核心的逻辑，原先内联在 5 处网络循环里无法单测。
/// 2. 修正性能 —— 原实现每行都从缓冲区头部 `position` 重新扫描，
///    拉取大模型时 NDJSON 行数可达数十万，扫描累计 O(n²)。
///    这里用 `scanned` 游标记录已扫描位置，使扫描均摊 O(1)。
///
/// 安全性：以 `\n`（0x0A）切分，而 0x0A 不会出现在 UTF-8 多字节序列内部，
/// 因此跨 chunk 的中文等内容不会被截断。
#[derive(Debug, Default)]
pub struct NdjsonReader {
    buf: Vec<u8>,
    /// 已扫描到的字节偏移，避免每行从头重扫
    scanned: usize,
}

impl NdjsonReader {
    pub fn new() -> Self {
        Self { buf: Vec::new(), scanned: 0 }
    }

    /// 追加一块网络数据
    pub fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// 取出下一个完整 JSON 对象。缓冲区中不足一行时返回 `Ok(None)`。
    /// 空行会被跳过；非法 JSON 返回 `Parse` 错误。
    pub fn next_value(&mut self) -> Result<Option<serde_json::Value>, OllamaError> {
        loop {
            let nl = match self.buf[self.scanned..].iter().position(|&b| b == b'\n') {
                Some(p) => self.scanned + p,
                None => {
                    // 尚未凑齐一整行，记录已扫描位置后等待更多数据
                    self.scanned = self.buf.len();
                    return Ok(None);
                }
            };

            let line = String::from_utf8_lossy(&self.buf[..nl]).trim().to_string();
            self.buf.drain(..=nl);
            self.scanned = 0;

            if line.is_empty() {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(&line)
                .map_err(|e| OllamaError::parse(format!("解析 NDJSON 失败: {}", e)))?;
            return Ok(Some(v));
        }
    }

    /// 消费缓冲区中当前所有完整行的便捷方法
    pub fn drain_values(&mut self) -> Result<Vec<serde_json::Value>, OllamaError> {
        let mut out = Vec::new();
        while let Some(v) = self.next_value()? {
            out.push(v);
        }
        Ok(out)
    }
}

/// Ollama 客户端，可取消的静态请求句柄由外部持有
#[derive(Debug, Clone)]
pub struct OllamaClient {
    pub base: String,
    pub http: reqwest::Client,
}

impl Default for OllamaClient {
    fn default() -> Self {
        Self::new("http://localhost:11434".to_string())
    }
}

impl OllamaClient {
    pub fn new(base: String) -> Self {
        // v1.0.9：全局共享 reqwest::Client（TLS 初始化与连接池只建一次）。
        // 此前每次 ollama::client() 都新建——3 秒轮询每次要建 3-4 个客户端，
        // 长时间运行造成连接池/句柄无谓增长。clone 仅 Arc 引用计数，开销可忽略。
        static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
        let http = HTTP.get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(2))
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .expect("构建 reqwest client 失败")
        });
        Self {
            base,
            http: http.clone(),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base.trim_end_matches('/'), path)
    }

    pub fn set_base(&mut self, base: String) {
        self.base = base;
    }

    /// 返回一个只用于长时间流式操作（pull/push）的客户端：连接超时 10s、整体请求超时 24h。
    /// 避免大模型在慢网络下被 10 分钟总超时切断，同时保留地址配置。
    pub fn long_timeout_clone(&self) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(24 * 60 * 60))
            .build()
            .expect("构建 reqwest client 失败");
        Self {
            base: self.base.clone(),
            http,
        }
    }
}

/// 读取用户配置中的服务地址构造客户端（空则回落 localhost:11434）。
/// 每次调用都取最新设置，因此改地址无需重启即生效。
pub fn client() -> OllamaClient {
    OllamaClient::new(crate::config::base_url())
}

/// 解析出「Ollama 实际使用的模型存储目录」。
///
/// 优先级与 Ollama 服务端一致：
/// 1. 应用设置里的 `OLLAMA_MODELS`（用户显式配置）
/// 2. 环境变量 `OLLAMA_MODELS`（服务端可能经 systemd 注入）
/// 3. `~/.ollama/models`（Ollama 默认值）
///
/// 返回 `None` 表示无法确定家目录（极端环境），调用方应静默跳过。
pub fn resolve_models_dir() -> Option<std::path::PathBuf> {
    let cfg = crate::config::Settings::load();
    if !cfg.ollama_models_dir.trim().is_empty() {
        return Some(std::path::PathBuf::from(cfg.ollama_models_dir.trim()));
    }
    if let Ok(env) = std::env::var("OLLAMA_MODELS") {
        if !env.trim().is_empty() {
            return Some(std::path::PathBuf::from(env.trim()));
        }
    }
    let home = std::env::var("HOME").ok()?;
    Some(std::path::PathBuf::from(home).join(".ollama").join("models"))
}

/// 判断文件名是否为 Ollama 的未完成分片。
///
/// 命名规则（仅这两种形态）：
/// - `sha256-<hex>-partial`      —— 主分片
/// - `sha256-<hex>-partial-<n>`  —— 并行分片，`<n>` 为十进制序号
///
/// **必须精确匹配**：不能用 `contains("-partial")`，否则 `foo-partialless`
/// 这类合法文件名会被误删（该缺陷由本模块单测在 v3.6.2 开发中发现并修复）。
fn is_partial_name(name: &str) -> bool {
    // 主分片：以 -partial 结尾
    if name.ends_with("-partial") {
        return true;
    }
    // 并行分片：-partial-<数字>，且数字后不得再有内容
    if let Some(rest) = name.rsplit_once("-partial-").map(|(_, r)| r) {
        return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit());
    }
    false
}

/// 清理模型目录里未完成的分片残片（`blobs/*-partial*`）。
///
/// 背景（v3.6.2，用户实测反复踩中）：Ollama 下载 blob 时先写
/// `sha256-<digest>-partial`，校验通过后改名为正式文件名。
/// 一旦下载被中止（关窗 / 断网 / 重启服务 / 进程被杀），这些半成品会**留在原地**；
/// 下次拉取同一个模型时 Ollama 撞上不一致的分片状态，底层直接断开
/// → 流内只回 `error="EOF"`（HTTP 仍是 200）→ 用户看到 `[Api] EOF`。
/// 这就是「下载经常在不同阶段失败」的直接原因。
///
/// 本函数只删 `-partial` 结尾的文件，**绝不触碰正式 blob 与 manifests**，
/// 因此已装好的模型零风险。返回被清理的文件数与释放的字节数。
pub fn clean_partial_blobs() -> Result<(usize, u64), String> {
    let models = resolve_models_dir().ok_or_else(|| "无法确定模型目录".to_string())?;
    let blobs = models.join("blobs");
    if !blobs.is_dir() {
        return Ok((0, 0));
    }
    let entries = std::fs::read_dir(&blobs).map_err(|e| format!("读取 {} 失败：{e}", blobs.display()))?;
    let mut count = 0usize;
    let mut freed = 0u64;
    let mut failed: Vec<String> = Vec::new();
    for ent in entries.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy();
        // 只认 Ollama 的分片命名，且必须**以 -partial 结尾**或后接 `-<数字>`：
        //   `sha256-<hex>-partial`        （主分片）
        //   `sha256-<hex>-partial-<n>`    （并行分片）
        // 注意：不能用 contains("-partial")——那会误伤 `xxx-partialless` 这类
        // 合法文件名（本函数的单测正是靠这条断言发现了该缺陷）。
        if !name.starts_with("sha256-") || !is_partial_name(&name) {
            continue;
        }
        let path = ent.path();
        let size = ent.metadata().map(|m| m.len()).unwrap_or(0);
        match std::fs::remove_file(&path) {
            Ok(()) => {
                count += 1;
                freed += size;
            }
            Err(e) => failed.push(format!("{}（{e}）", path.display())),
        }
    }
    if !failed.is_empty() {
        // 部分失败：如服务账户创建的残片当前用户删不掉，需提权
        return Err(format!(
            "已清理 {count} 个残留分片，但有 {} 个删除失败：\n{}",
            failed.len(),
            failed.join("\n")
        ));
    }
    Ok((count, freed))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- human_size ----------

    #[test]
    fn human_size_bytes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1), "1 B");
        assert_eq!(human_size(1023), "1023 B");
    }

    #[test]
    fn human_size_boundaries() {
        assert_eq!(human_size(1024), "1.00 KB");
        assert_eq!(human_size(1024 * 1024), "1.00 MB");
        assert_eq!(human_size(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(human_size(1024u64.pow(4)), "1.00 TB");
    }

    #[test]
    fn human_size_fractional() {
        assert_eq!(human_size(1536), "1.50 KB"); // 1.5 KB
        assert_eq!(human_size(1024 * 1024 * 3 / 2), "1.50 MB");
    }

    #[test]
    fn human_size_does_not_overflow_unit_table() {
        // u64::MAX 约 16EB，单位表到 TB 为止，索引不应越界 panic
        assert_eq!(human_size(u64::MAX), "16777216.00 TB");
    }

    #[test]
    fn human_size_rounds_down_not_up_at_1023_95() {
        // 回归防护：1023.95 KB 应显示 1023.95 而非被四舍五入成 1024.00
        let v = 1024 * 1024 - 51; // ≈ 1023.95 KB
        assert_eq!(human_size(v), "1023.95 KB");
    }

    // ---------- GenStats ----------

    #[test]
    fn stats_empty_is_detected() {
        // 全零统计不应在界面上渲染出统计行
        let s = GenStats::default();
        assert!(s.is_empty());
        assert_eq!(s.summary(), "");
    }

    #[test]
    fn stats_eval_tokens_per_sec() {
        let s = GenStats {
            eval_count: 100,
            eval_duration_ns: 4_000_000_000, // 4 秒
            ..GenStats::default()
        };
        assert_eq!(s.eval_tokens_per_sec(), 25.0);
    }

    #[test]
    fn stats_division_by_zero_is_safe() {
        // 某些模型/版本不回传耗时，除零不能 panic
        let s = GenStats { eval_count: 10, ..GenStats::default() };
        assert_eq!(s.eval_tokens_per_sec(), 0.0);
        assert_eq!(s.prompt_tokens_per_sec(), 0.0);
    }

    #[test]
    fn stats_summary_includes_tokens_speed_and_prompt() {
        let s = GenStats {
            total_duration_ns: 5_200_000_000,
            prompt_eval_count: 42,
            prompt_eval_duration_ns: 500_000_000,
            eval_count: 128,
            eval_duration_ns: 4_000_000_000,
            tool_calls: None,
        };
        let out = s.summary();
        assert!(out.contains("128 tokens"), "缺少 token 数: {out}");
        assert!(out.contains("32.0 tokens/s"), "缺少速度: {out}");
        assert!(out.contains("5.20s"), "缺少总耗时: {out}");
        assert!(out.contains("提示 42 tokens"), "缺少提示 token 数: {out}");
    }

    #[test]
    fn extract_stats_reads_final_frame() {
        let v = serde_json::json!({
            "done": true,
            "total_duration": 5_200_000_000u64,
            "prompt_eval_count": 42,
            "prompt_eval_duration": 500_000_000u64,
            "eval_count": 128,
            "eval_duration": 4_000_000_000u64
        });
        let s = extract_stats(&v);
        assert_eq!(s.eval_count, 128);
        assert_eq!(s.prompt_eval_count, 42);
        assert_eq!(s.total_duration_ns, 5_200_000_000);
        assert!(!s.is_empty());
    }

    #[test]
    fn extract_stats_tolerates_float_durations_and_missing_fields() {
        // 不同 Ollama 版本可能把时长作为浮点返回，缺字段应回落 0 而不是 panic
        let v = serde_json::json!({"done": true, "eval_duration": 4.0e9, "eval_count": 8});
        let s = extract_stats(&v);
        assert_eq!(s.eval_duration_ns, 4_000_000_000);
        assert_eq!(s.total_duration_ns, 0, "缺失字段应回落 0");
    }

    #[test]
    fn extract_stats_reads_tool_calls() {
        // Ollama 原生形态：message.tool_calls[].function.{name,arguments}
        let v = serde_json::json!({
            "done": true,
            "eval_count": 4,
            "message": {
                "content": "",
                "tool_calls": [{
                    "function": {
                        "name": "get_weather",
                        "arguments": {"city": "广州", "unit": "celsius"}
                    }
                }]
            }
        });
        let s = extract_stats(&v);
        assert!(s.tool_calls.is_some(), "应捕获工具调用");
        let text = s.tool_calls_text();
        assert!(text.contains("get_weather"), "应显示函数名: {text}");
        assert!(text.contains("广州"), "应显示参数: {text}");
        assert!(text.contains("函数调用"), "应有标题: {text}");
    }

    #[test]
    fn tool_calls_text_empty_when_none() {
        let s = GenStats::default();
        assert_eq!(s.tool_calls_text(), "", "无工具调用应为空串");
    }

    // ---------- OllamaClient::url ----------

    #[test]
    fn url_joins_path() {
        let c = OllamaClient::default();
        assert_eq!(c.url("/api/tags"), "http://localhost:11434/api/tags");
    }

    #[test]
    fn url_strips_trailing_slash_from_base() {
        // 用户在设置里填 "http://host:11434/" 时不应产生 "//api/tags"
        let c = OllamaClient::new("http://192.168.1.10:11434/".into());
        assert_eq!(c.url("/api/tags"), "http://192.168.1.10:11434/api/tags");
    }

    #[test]
    fn url_strips_multiple_trailing_slashes() {
        let c = OllamaClient::new("http://localhost:11434///".into());
        assert_eq!(c.url("/api/ps"), "http://localhost:11434/api/ps");
    }

    #[test]
    fn set_base_takes_effect() {
        let mut c = OllamaClient::default();
        c.set_base("http://10.0.0.5:11434".into());
        assert_eq!(c.url("/api/version"), "http://10.0.0.5:11434/api/version");
    }

    // ---------- GenParams::to_option ----------

    fn empty_params() -> GenParams {
        GenParams::default()
    }

    #[test]
    fn gen_params_all_none_yields_empty_object() {
        // 全 None 时不应把 null 传给 Ollama，否则会覆盖服务端默认参数
        assert_eq!(empty_params().to_option(), serde_json::json!({}));
    }

    #[test]
    fn gen_params_only_includes_set_fields() {
        let p = GenParams {
            temperature: Some(0.8),
            num_ctx: Some(2048),
            ..empty_params()
        };
        let v = p.to_option();
        // 注意：temperature 是 f32，序列化到 f64 JSON 后为 0.800000011920929，
        // 因此用容差比较。这个精度损失是真实存在的行为（见 KNOWN_ISSUES）。
        let t = v["temperature"].as_f64().expect("temperature 应为数字");
        assert!((t - 0.8).abs() < 1e-6, "temperature 偏差过大: {}", t);
        assert_eq!(v["num_ctx"], serde_json::json!(2048));
        assert!(v.get("top_p").is_none());
        assert!(v.get("seed").is_none());
        assert_eq!(v.as_object().unwrap().len(), 2);
    }

    #[test]
    fn gen_params_includes_tools_at_top_level() {
        // Ollama 的 tools 是请求体顶层数组，不是 options 子对象
        let p = GenParams {
            tools: Some(vec![serde_json::json!({
                "type": "function",
                "function": {"name": "get_weather", "description": "获取天气", "parameters": {}}
            })]),
            ..empty_params()
        };
        let v = p.to_option();
        assert_eq!(
            v["tools"],
            serde_json::json!([{
                "type": "function",
                "function": {"name": "get_weather", "description": "获取天气", "parameters": {}}
            }]),
            "tools 应原样放在顶层"
        );
        assert!(p.options_value().is_none(), "tools 不应进入 options");
    }

    #[test]
    fn gen_params_empty_tools_is_omitted() {
        // 空数组与 None 都不应把 tools 发给 Ollama
        let none = empty_params();
        assert!(none.to_option().get("tools").is_none());
        let empty = GenParams { tools: Some(vec![]), ..empty_params() };
        assert!(empty.to_option().get("tools").is_none());
    }

    #[test]
    fn gen_params_f32_precision_artifact() {
        // 记录现状：f32 存储导致发送给 Ollama 的参数存在精度尾数。
        // 若日后把 GenParams 的浮点字段改为 f64，此测试会失败，
        // 届时说明精度问题已修复，可更新断言为精确等于 0.8。
        let p = GenParams { temperature: Some(0.8), ..empty_params() };
        let v = p.to_option();
        let t = v["temperature"].as_f64().unwrap();
        assert_ne!(t, 0.8f64, "f32 应产生精度尾数；若失败说明已改为 f64");
        assert!((t - 0.8).abs() < 1e-6);
    }

    #[test]
    fn gen_params_includes_system() {
        let p = GenParams {
            system: Some("你是一个助手".into()),
            ..empty_params()
        };
        assert_eq!(p.to_option()["system"], serde_json::json!("你是一个助手"));
    }

    // ---------- NdjsonReader ----------

    #[test]
    fn ndjson_single_line() {
        let mut r = NdjsonReader::new();
        r.feed(b"{\"a\":1}\n");
        assert_eq!(r.next_value().unwrap(), Some(serde_json::json!({"a": 1})));
        assert_eq!(r.next_value().unwrap(), None);
    }

    #[test]
    fn ndjson_multiple_lines_in_one_chunk() {
        let mut r = NdjsonReader::new();
        r.feed(b"{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n");
        let vals = r.drain_values().unwrap();
        assert_eq!(vals.len(), 3);
        assert_eq!(vals[2]["a"], serde_json::json!(3));
    }

    #[test]
    fn ndjson_line_split_across_chunks() {
        // 网络分片的真实情况：一行 JSON 被切在多个 chunk 里
        let mut r = NdjsonReader::new();
        r.feed(b"{\"mess");
        assert_eq!(r.next_value().unwrap(), None, "未凑齐整行时不应产出");
        r.feed(b"age\":{\"content\":\"hi\"}}\n");
        assert_eq!(
            r.next_value().unwrap(),
            Some(serde_json::json!({"message": {"content": "hi"}}))
        );
    }

    #[test]
    fn ndjson_utf8_split_mid_multibyte_sequence() {
        // 中文字符的 UTF-8 字节被切开，重组后必须完整（这是原实现的潜在雷区）
        let full = "{\"message\":{\"content\":\"你好\"}}\n";
        let bytes = full.as_bytes();
        let mut r = NdjsonReader::new();
        // 在任意位置切一刀都应当能正确重组
        for cut in 1..bytes.len() {
            let mut r2 = NdjsonReader::new();
            r2.feed(&bytes[..cut]);
            r2.feed(&bytes[cut..]);
            assert_eq!(
                r2.next_value().unwrap(),
                Some(serde_json::json!({"message": {"content": "你好"}})),
                "在字节 {} 处切分后重组失败",
                cut
            );
        }
        let _ = &mut r;
    }

    #[test]
    fn ndjson_skips_blank_lines() {
        let mut r = NdjsonReader::new();
        r.feed(b"\n\n{\"a\":1}\n\n\n{\"a\":2}\n\n");
        let vals = r.drain_values().unwrap();
        assert_eq!(vals.len(), 2, "空行应被跳过");
    }

    #[test]
    fn ndjson_rejects_malformed_json() {
        let mut r = NdjsonReader::new();
        r.feed(b"this is not json\n");
        let err = r.next_value().unwrap_err();
        match err.kind {
            ErrorKind::Parse => {}
            other => panic!("期望 Parse 错误，实际 {:?}", other),
        }
    }

    #[test]
    fn ndjson_handles_trailing_partial_line() {
        // 流结束但最后一行没有换行符：不应丢失已完整的行
        let mut r = NdjsonReader::new();
        r.feed(b"{\"a\":1}\n{\"a\":2}");
        let vals = r.drain_values().unwrap();
        assert_eq!(vals.len(), 1, "只有第一行完整");
        assert_eq!(vals[0]["a"], serde_json::json!(1));
    }

    #[test]
    fn ndjson_scanned_cursor_prevents_rescan() {
        // 逐字节喂入，每次都只有最后一个 chunk 含换行符。
        // 若实现退化为「每行从头扫描」，这里会因 O(n²) 而明显变慢。
        let payload: Vec<u8> = b"{\"x\":0}\n".repeat(500);
        let mut r = NdjsonReader::new();
        for b in &payload {
            r.feed(&[*b]);
        }
        assert_eq!(r.drain_values().unwrap().len(), 500);
    }
}

#[cfg(test)]
mod api_error_tests {
    use super::*;

    /// 复现用户截图里的注册中心 403：应映射为可行动中文提示，
    /// 而不是把原始 JSON（含 ANONYMOUS_ACCESS_DENIED）丢给用户。
    #[test]
    fn api_from_body_maps_anonymous_access_denied() {
        let body = r#"{"errors":[{"code":"ANONYMOUS_ACCESS_DENIED","message":"anonymous access denied"}]}"#;
        let e = OllamaError::api_from_body(reqwest::StatusCode::FORBIDDEN, body);
        assert_eq!(e.kind, ErrorKind::Api);
        assert!(
            e.message.contains("ollama login") || e.message.contains("登录"),
            "应给出可行动提示，实际: {}",
            e.message
        );
        assert!(
            !e.message.contains("ANONYMOUS_ACCESS_DENIED"),
            "不应把原始错误码直接展示，实际: {}",
            e.message
        );
    }

    /// 锁定 StatusCode 的 Display 形态：兜底分支靠它，改动可被立即发现。
    #[test]
    fn status_code_display_shape() {
        let s = format!("{}", reqwest::StatusCode::FORBIDDEN);
        println!("StatusCode FORBIDDEN Display = {s:?}");
        assert!(s.contains("403"), "应含状态码，实际: {s}");
    }

    /// 非 JSON 响应体走兜底分支，不能崩、要带上状态码与原文。
    #[test]
    fn api_from_body_falls_back_on_non_json() {
        let e = OllamaError::api_from_body(reqwest::StatusCode::BAD_GATEWAY, "<html>502</html>");
        assert_eq!(e.kind, ErrorKind::Api);
        assert!(e.message.contains("502"));
        assert!(e.message.contains("<html>"));
    }

    /// HTTP 403 但体里没有可识别错误码时，仍给可行动提示，不抛原始 JSON。
    #[test]
    fn api_from_body_403_without_code_gives_hint() {
        let e = OllamaError::api_from_body(reqwest::StatusCode::FORBIDDEN, "{\"detail\":\"nope\"}");
        assert!(e.message.contains("ollama login"), "实际: {}", e.message);
    }

    /// 用户截图里的真实形态：Ollama 返回 200，失败原因在流内 error 字段，
    /// 形如 `403: {"errors":[{"code":"ANONYMOUS_ACCESS_DENIED",...}]}`。
    /// 旧实现只翻译 HTTP 非 2xx 分支，对推送永不触发 → 用户仍看到原始 JSON。
    #[test]
    fn stream_error_maps_registry_denied() {
        let raw = r#"403: {"errors":[{"code":"ANONYMOUS_ACCESS_DENIED","message":"anonymous access denied"}]}"#;
        let e = OllamaError::api_from_stream_error(raw);
        assert_eq!(e.kind, ErrorKind::Api);
        assert!(e.message.contains("ollama login"), "实际: {}", e.message);
        assert!(!e.message.contains("ANONYMOUS_ACCESS_DENIED"), "不应外泄原始错误码");
        assert!(!e.message.starts_with("403"), "不应以状态码开头");
    }

    /// 流内只有小写 message、没有 code 字段时也要识别
    #[test]
    fn stream_error_maps_lowercase_denied() {
        let e = OllamaError::api_from_stream_error("403: anonymous access denied");
        assert!(e.message.contains("ollama login"), "实际: {}", e.message);
    }

    /// 普通模型报错不得被误判成登录问题
    #[test]
    fn stream_error_passthrough_normal_error() {
        let e = OllamaError::api_from_stream_error("model 'foo:bar' not found, try pulling it first");
        assert!(e.message.contains("not found"), "实际: {}", e.message);
        assert!(!e.message.contains("ollama login"));
    }

    // ---------- 裸 EOF 翻译（v3.6.2，用户实测反复踩中）----------

    /// 用户截图里的真实形态：流内只回 `error="EOF"`（HTTP 仍 200）。
    /// 旧实现原文透传 → 界面显示 `[Api] EOF`，三个字母毫无信息量。
    #[test]
    fn stream_error_bare_eof_gives_actionable_hint() {
        let e = OllamaError::api_from_stream_error("EOF");
        assert_eq!(e.kind, ErrorKind::Api);
        assert_ne!(e.message.trim(), "EOF", "不应原文透传");
        assert!(
            e.message.contains("残留") || e.message.contains("分片"),
            "应解释残留分片成因，实际: {}",
            e.message
        );
        assert!(
            e.message.contains("重试") || e.message.contains("修复"),
            "应给出下一步动作，实际: {}",
            e.message
        );
        assert!(
            e.message.contains("不受影响") || e.message.contains("无需担心"),
            "应安抚用户「已有模型没坏」，实际: {}",
            e.message
        );
    }

    /// `unexpected EOF`、带前缀的 EOF 变体都要识别
    #[test]
    fn stream_error_eof_variants() {
        for raw in ["unexpected EOF", "unexpected eof", "  EOF  ", "EOF.", "read tcp: EOF"] {
            let e = OllamaError::api_from_stream_error(raw);
            assert_ne!(e.message.trim(), raw, "未识别 EOF 变体: {raw:?}");
            assert!(e.message.contains("分片"), "变体 {raw:?} 未走 EOF 提示: {}", e.message);
        }
    }

    /// `is_bare_eof` 的边界：非 EOF 内容一律不得命中
    #[test]
    fn is_bare_eof_rejects_other_errors() {
        assert!(!is_bare_eof("model not found"));
        assert!(!is_bare_eof(""));
        assert!(!is_bare_eof("403 Forbidden"));
        assert!(!is_bare_eof("connection reset by peer"));
        // 含 EOF 但另有实质信息的（如带状态码）不走 EOF 分支
        assert!(!is_bare_eof("EOF while parsing registry response: code=DENIED"));
    }

    // ---------- 残留分片清理的安全性（v3.6.2 核心）----------

    /// `is_partial_name` 的精确匹配边界——
    /// 开发期本模块单测正是抓到 `contains("-partial")` 会误删 `xxx-partialless`。
    #[test]
    fn is_partial_name_matches_only_real_partials() {
        // 真分片
        assert!(is_partial_name("sha256-abc123-partial"));
        assert!(is_partial_name("sha256-abc123-partial-0"));
        assert!(is_partial_name("sha256-abc123-partial-12"));
        // 非分片（含陷阱名）——一律不得命中
        assert!(!is_partial_name("sha256-abc123-partialless"));
        assert!(!is_partial_name("sha256-abc123"));
        assert!(!is_partial_name("README.txt"));
        assert!(!is_partial_name("sha256-abc-partial-"));
        assert!(!is_partial_name("sha256-abc-partial-x"));
        assert!(!is_partial_name("sha256-abc-partial-1.bak"));
        assert!(!is_partial_name("partial"));
    }

    /// 在隔离目录里验证 `clean_partial_blobs`：
    /// **只删 `-partial*`，正式 blob 与无关文件绝不能误伤**。
    /// 这条边界直接关系到用户既有模型的安全，必须有测试守护。
    #[test]
    fn clean_partial_blobs_only_removes_partials() {
        // 本测试修改进程级环境变量（HOME / OLLAMA_MODELS），
        // 必须与同样依赖环境变量的测试串行，避免并行竞态。
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let root = std::env::temp_dir().join(format!("offidesk-clean-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // 造出标准布局 <fake_home>/.ollama/models/blobs
        let fake_home = root.join("home");
        let blobs = fake_home.join(".ollama/models/blobs");
        std::fs::create_dir_all(&blobs).unwrap();

        // 正式 blob（必须保留）
        std::fs::write(blobs.join("sha256-aaaa-partialless"), vec![0u8; 100]).unwrap();
        // 未完成分片（必须删除）：500 + 10 + 20 = 530 字节
        std::fs::write(blobs.join("sha256-bbbb-partial"), vec![0u8; 500]).unwrap();
        std::fs::write(blobs.join("sha256-bbbb-partial-0"), vec![0u8; 10]).unwrap();
        std::fs::write(blobs.join("sha256-cccc-partial-3"), vec![0u8; 20]).unwrap();
        // 无关文件（必须保留）
        std::fs::write(blobs.join("README.txt"), b"hello").unwrap();

        // 通过 HOME 指向隔离目录（resolve_models_dir 的兜底分支），不动真实模型
        let old_home = std::env::var("HOME").ok();
        let old_models = std::env::var("OLLAMA_MODELS").ok();
        std::env::set_var("HOME", &fake_home);
        std::env::remove_var("OLLAMA_MODELS");

        let (n, freed) = clean_partial_blobs().expect("清理应成功");
        assert_eq!(n, 3, "应删除 3 个残留分片");
        assert_eq!(freed, 530, "释放字节应为 500+10+20");

        let remain: Vec<String> = std::fs::read_dir(&blobs)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            remain.iter().any(|f| f == "sha256-aaaa-partialless"),
            "正式 blob 被误删！剩余: {remain:?}"
        );
        assert!(remain.iter().any(|f| f == "README.txt"), "无关文件被误删！");
        assert!(
            !remain.iter().any(|f| is_partial_name(f)),
            "仍有分片残留: {remain:?}"
        );

        // 幂等：再清理应为 0
        let (n2, f2) = clean_partial_blobs().expect("二次清理应成功");
        assert_eq!((n2, f2), (0, 0), "无残片时应返回 (0,0)");

        // blobs 目录不存在时应安全返回，而非报错
        std::env::set_var("HOME", root.join("nohome"));
        let (n3, f3) = clean_partial_blobs().expect("目录不存在不应报错");
        assert_eq!((n3, f3), (0, 0));

        // 还原环境
        if let Some(h) = old_home {
            std::env::set_var("HOME", h);
        }
        if let Some(m) = old_models {
            std::env::set_var("OLLAMA_MODELS", m);
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
