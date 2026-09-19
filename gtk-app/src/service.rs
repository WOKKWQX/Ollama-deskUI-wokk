//! Ollama 服务启停控制
//!
//! 设计要点（针对历史上「启停按键无效」的三个根因）：
//!
//! 1. **全部为同步函数**。状态检测与启停会被放进 `rt::spawn` 的 future 里执行，而
//!    `rt::spawn` 的 future 本身运行在 `runtime().block_on()` 的上下文中。此前
//!    `is_running()` 内部又调用 `rt::block_on`，触发 tokio panic
//!    （Cannot start a runtime from within a runtime），后台线程静默死亡，
//!    UI 回调永不触发 → 按钮状态卡死。这里改用纯同步的 TCP 探测，杜绝嵌套 runtime。
//!
//! 2. **以真实探测为准**，不依赖控件状态推导。systemd 与 API 任一为真即视为运行中，
//!    满足「全场景流程实时反映本地真实状态」。
//!
//! 3. **systemctl 需要 root**，普通用户直接调用必然失败。故按
//!    `systemctl` → `pkexec systemctl` → 直接 spawn `ollama serve` 逐级降级，
//!    保证在没有 systemd 单元或没有 root 权限的机器上同样可用。

use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::time::Duration;

/// 从 base_url 纯函数式地解析出 `host:port`（便于单测）
fn parse_host_port(base: &str) -> String {
    let s = base.trim();
    let s = s
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let hostport = s.split('/').next().unwrap_or("").trim();
    if hostport.is_empty() {
        "127.0.0.1:11434".to_string()
    } else if hostport.contains(':') {
        hostport.to_string()
    } else {
        // 未写端口时补 Ollama 默认端口
        format!("{hostport}:11434")
    }
}

/// 当前配置解析出的 host:port
fn host_port() -> String {
    parse_host_port(&crate::config::base_url())
}

/// 当前 Ollama 服务地址是否指向本机（localhost / 127.0.0.1 / ::1）。
/// 启停操作只能作用于本机服务；远程地址时应由 UI 禁用或提示（v1.0.7）。
pub fn is_local_target() -> bool {
    let hp = host_port();
    is_local_host(hp.rsplit_once(':').map(|(h, _)| h).unwrap_or(hp.as_str()))
}

/// 纯判定，供单测。
fn is_local_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1" | "::")
}

/// 同步探测 Ollama API 是否可达（TCP 连接，800ms 超时）。
/// 纯同步、不触碰 tokio，可安全地在任意线程或异步上下文中调用。
pub fn api_reachable() -> bool {
    let hp = host_port();
    match hp.to_socket_addrs() {
        Ok(addrs) => addrs
            .into_iter()
            .any(|a| TcpStream::connect_timeout(&a, Duration::from_millis(800)).is_ok()),
        Err(_) => false,
    }
}

/// systemd 中是否存在 ollama.service 单元（引擎管理模块 engine.rs 复用）
pub(crate) fn has_systemd_unit() -> bool {
    Command::new("systemctl")
        .args(["list-unit-files", "ollama.service"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|l| l.trim_start().starts_with("ollama.service"))
        })
        .unwrap_or(false)
}

/// systemd 是否报告 ollama 处于 active
fn systemd_active() -> bool {
    Command::new("systemctl")
        .args(["is-active", "ollama"])
        .output()
        .map(|o| {
            o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "active"
        })
        .unwrap_or(false)
}

/// 服务是否在运行（systemd active 或 API 可达，任一为真）
pub fn is_running() -> bool {
    systemd_active() || api_reachable()
}

/// 当前是由 systemd 托管，还是用户自行起的进程
pub fn managed_by() -> &'static str {
    if systemd_active() {
        "systemd"
    } else if api_reachable() {
        "进程"
    } else {
        "未运行"
    }
}

fn systemctl(op: &str) -> Result<(), String> {
    let st = Command::new("systemctl")
        .args([op, "ollama"])
        .status()
        .map_err(|e| format!("systemctl {op} 执行失败: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("systemctl {op} ollama 返回失败（可能需要管理员权限）"))
    }
}

fn pkexec_systemctl(op: &str) -> Result<(), String> {
    let st = Command::new("pkexec")
        .args(["systemctl", op, "ollama"])
        .status()
        .map_err(|e| format!("pkexec systemctl {op} 失败: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("授权失败或 systemctl {op} 失败"))
    }
}

/// 直接以独立会话启动 `ollama serve`（脱离本进程，避免随应用退出被杀）
fn spawn_serve() -> Result<(), String> {
    let has_setsid = Command::new("sh")
        .args(["-c", "command -v setsid"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let mut cmd = if has_setsid {
        let mut c = Command::new("setsid");
        c.arg("ollama").arg("serve");
        c
    } else {
        let mut c = Command::new("ollama");
        c.arg("serve");
        c
    };

    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // 注入用户配置的服务端全局环境变量（设置页「服务端配置」分组）。
    // 仅当字段非空/非零时才写入，空值回落为服务端自身默认，避免覆盖。
    // 这些变量在 ollama serve 启动后由服务端读取，因此改动后需重启服务生效。
    let s = crate::config::Settings::load();
    if !s.ollama_models_dir.is_empty() {
        cmd.env("OLLAMA_MODELS", &s.ollama_models_dir);
    }
    if !s.default_keep_alive.is_empty() {
        cmd.env("OLLAMA_KEEP_ALIVE", &s.default_keep_alive);
    }
    if s.num_parallel > 0 {
        cmd.env("OLLAMA_NUM_PARALLEL", s.num_parallel.to_string());
    }
    if s.max_loaded_models > 0 {
        cmd.env("OLLAMA_MAX_LOADED_MODELS", s.max_loaded_models.to_string());
    }
    if !s.gpu_visible.is_empty() {
        cmd.env("OLLAMA_GPU", &s.gpu_visible);
        cmd.env("CUDA_VISIBLE_DEVICES", &s.gpu_visible);
    }

    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 ollama serve 失败: {e}（请确认已安装 ollama 且位于 PATH 中）"))
}

/// 结束用户自行启动的 ollama serve 进程
fn kill_serve() -> Result<(), String> {
    let st = Command::new("pkill")
        .args(["-f", "ollama serve"])
        .status()
        .map_err(|e| format!("pkill 失败: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err("未找到正在运行的 ollama serve 进程".into())
    }
}

/// 等待服务起来（最多约 5 秒）。返回是否真的就绪。
///
/// v3.6.1：由 `()` 改为返回 bool。旧实现里 `systemctl start` 退出码为 0 即
/// `return Ok(())`，但 systemd 的 `Restart=always` 会让「启动命令成功、进程随即
/// 反复崩溃」也返回成功——用户点「启动服务」看到无错误提示，实际服务根本没起来，
/// 表现为「运行欧拉玛失效」。现在必须轮询确认真的就绪才算成功。
fn wait_up() -> bool {
    for _ in 0..20 {
        if is_running() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// 等待服务停止（最多约 5 秒）
fn wait_down() {
    for _ in 0..20 {
        if !is_running() {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// 启动服务。已运行则直接成功。
pub fn start() -> Result<(), String> {
    if is_running() {
        return Ok(());
    }

    // 逐级降级，并把每种尝试的失败原因都留下来，
    // 最终一次性如实呈现给用户（便于判断是缺权限还是没装 ollama）
    let mut attempts: Vec<String> = Vec::new();

    if has_systemd_unit() {
        match systemctl("start") {
            Ok(()) => {
                if wait_up() {
                    return Ok(());
                }
                attempts.push(
                    "systemctl start 执行成功，但 5 秒内服务未就绪（服务可能启动即崩溃，\
                     可执行 `journalctl -u ollama -n 50` 查看原因）"
                        .into(),
                );
            }
            Err(e) => attempts.push(e),
        }
        match pkexec_systemctl("start") {
            Ok(()) => {
                if wait_up() {
                    return Ok(());
                }
                attempts.push(
                    "pkexec systemctl start 执行成功，但 5 秒内服务未就绪\
                     （可执行 `journalctl -u ollama -n 50` 查看原因）"
                        .into(),
                );
            }
            Err(e) => attempts.push(e),
        }
    }

    match spawn_serve() {
        Ok(()) => {
            if wait_up() {
                return Ok(());
            }
            attempts.push(
                "已执行 `ollama serve`，但服务未就绪（可尝试在终端手动执行该命令确认）".into(),
            );
        }
        Err(e) => attempts.push(e),
    }

    if attempts.is_empty() {
        attempts.push("未找到 ollama.service 单元，且无法启动 ollama serve".into());
    }
    Err(format!("启动失败，已依次尝试：\n• {}", attempts.join("\n• ")))
}

/// 停止服务。未运行则直接成功。
pub fn stop() -> Result<(), String> {
    if !is_running() {
        return Ok(());
    }

    let mut attempts: Vec<String> = Vec::new();

    if has_systemd_unit() && systemd_active() {
        match systemctl("stop") {
            Ok(()) => {
                wait_down();
                return Ok(());
            }
            Err(e) => attempts.push(e),
        }
        match pkexec_systemctl("stop") {
            Ok(()) => {
                wait_down();
                return Ok(());
            }
            Err(e) => attempts.push(e),
        }
    }

    match kill_serve() {
        Ok(()) => {
            wait_down();
            return Ok(());
        }
        Err(e) => attempts.push(e),
    }

    if attempts.is_empty() {
        attempts.push("未找到可停止的服务或进程".into());
    }
    Err(format!("停止失败，已依次尝试：\n• {}", attempts.join("\n• ")))
}

/// 读取 Ollama 服务日志原文，供「服务日志」窗展示。
///
/// 读取顺序（取第一个有内容的来源，如实告知用户哪种来源为空）：
/// 1. `journalctl -u ollama`（systemd 托管时最权威）
/// 2. `~/.ollama/logs/server.log`（部分安装会把日志写入此处）
/// 3. 都没有则给出解释性提示，而不是假装空日志是正常。
pub fn read_logs() -> String {
    // 1) journalctl
    if let Ok(o) = Command::new("journalctl")
        .args(["-u", "ollama", "--no-pager", "-n", "800"])
        .output()
    {
        let s = String::from_utf8_lossy(&o.stdout);
        if !s.trim().is_empty() {
            return format!("（来源：journalctl -u ollama）\n\n{s}");
        }
    }
    // 2) 日志文件
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let log_path = std::path::Path::new(&home)
        .join(".ollama")
        .join("logs")
        .join("server.log");
    if let Ok(s) = std::fs::read_to_string(&log_path) {
        if !s.trim().is_empty() {
            return format!("（来源：{}）\n\n{}", log_path.display(), s);
        }
    }
    // 3) 都没有
    format!(
        "未找到 Ollama 日志内容。\n\n常见原因：\n\
        • 若 Ollama 以 systemd 运行：请确认 journald 可用，或终端执行 `journalctl -u ollama -n 800` 自查。\n\
        • 若以 `ollama serve` 直接运行（本程序降级启动方式）：日志输出到启动它的终端，未落盘。\n\
        • 日志路径 {} 不存在或为空。",
        log_path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::{is_local_host, parse_host_port};

    #[test]
    fn parses_default_url() {
        assert_eq!(parse_host_port("http://localhost:11434"), "localhost:11434");
        assert_eq!(parse_host_port("http://127.0.0.1:11434"), "127.0.0.1:11434");
    }

    #[test]
    fn parses_https_and_trailing_path() {
        assert_eq!(parse_host_port("https://ollama.example.com/api"), "ollama.example.com:11434");
        assert_eq!(parse_host_port("http://10.0.0.5:1234/"), "10.0.0.5:1234");
    }

    #[test]
    fn adds_default_port_when_missing() {
        // 用户只填了主机名，应补上 Ollama 默认端口，否则 TCP 探测必然失败
        assert_eq!(parse_host_port("http://myhost"), "myhost:11434");
        assert_eq!(parse_host_port("myhost"), "myhost:11434");
    }

    #[test]
    fn empty_url_falls_back_to_loopback() {
        assert_eq!(parse_host_port(""), "127.0.0.1:11434");
        assert_eq!(parse_host_port("   "), "127.0.0.1:11434");
    }

    #[test]
    fn trims_whitespace() {
        assert_eq!(parse_host_port("  http://localhost:11434  "), "localhost:11434");
    }

    #[test]
    fn bracketed_ipv6_kept_verbatim() {
        // 括号 IPv6：含 ':' 走原样保留分支，to_socket_addrs 可解析 "[::1]:11434"
        assert_eq!(parse_host_port("http://[::1]:11434"), "[::1]:11434");
    }

    #[test]
    fn userinfo_url_document_current_behavior() {
        // 已知缺口：带认证信息的 URL 会把 user:pass@host 当 host:port，
        // TCP 探测 to_socket_addrs 必然失败 → 状态灯判「停止」但 HTTP 请求可能成功（reqwest 支持 URL 认证）
        assert_eq!(parse_host_port("http://user:pass@h:11434"), "user:pass@h:11434");
    }

    #[test]
    fn local_host_detection() {
        // is_local_host 纯判定：环回地址 = 本机；其余（含内网 IP 与普通主机名）= 远程
        assert!(is_local_host("localhost"));
        assert!(is_local_host("127.0.0.1"));
        assert!(is_local_host("[::1]"));
        assert!(!is_local_host("192.168.1.5"));
        assert!(!is_local_host("myhost"));
        assert!(!is_local_host("user:pass@h"));
    }

    #[test]
    fn is_running_is_sync_and_safe_inside_runtime() {
        // 回归防护：`is_running()` 必须是**纯同步**实现。
        // 历史 bug：它内部调用 `rt::block_on`，而调用方本身运行在
        // `runtime().block_on()` 的上下文中，于是触发 tokio panic
        // 「Cannot start a runtime from within a runtime」，后台线程静默死亡、
        // UI 回调永不触发 → 启停按钮状态永远卡在初始值。
        // 这里在真实的 tokio runtime 上下文中调用，复现该场景：若改为异步实现就会 panic。
        let rt = tokio::runtime::Runtime::new().expect("构建测试 runtime");
        rt.block_on(async {
            let _ = super::is_running();
            let _ = super::api_reachable();
            let _ = super::managed_by();
        });
    }
}
