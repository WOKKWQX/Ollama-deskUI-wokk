//! 全局配置（设置项持久化 + 当前 Ollama 服务地址解析）
//! 独立于 ui 与 ollama 模块，避免循环依赖：
//! ollama 网络层通过 `base_url()` 读取用户配置，ui::settings 复用同一 Settings。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    /// Ollama 服务地址，空或缺失时回落为 http://localhost:11434
    pub base_url: String,
    /// 主题模式："system" | "dark" | "light"
    pub theme: String,
    /// 关闭窗口时最小化到托盘
    pub close_to_tray: bool,
    /// 已加载模型自动刷新间隔（秒）
    pub refresh_interval: u32,
    /// 服务端模型存储目录（对应 OLLAMA_MODELS），空 = 服务端默认
    #[serde(default)]
    pub ollama_models_dir: String,
    /// 默认 keep_alive（对应 OLLAMA_KEEP_ALIVE），空 = 服务端默认
    #[serde(default)]
    pub default_keep_alive: String,
    /// 并发请求数（OLLAMA_NUM_PARALLEL），0 = 服务端默认
    #[serde(default)]
    pub num_parallel: u32,
    /// 并行加载模型数（OLLAMA_MAX_LOADED_MODELS），0 = 服务端默认
    #[serde(default)]
    pub max_loaded_models: u32,
    /// 可见 GPU（OLLAMA_GPU / CUDA_VISIBLE_DEVICES），空 = 全部
    #[serde(default)]
    pub gpu_visible: String,
    /// 开机自启动（登录后静默进托盘，不弹主窗口）
    #[serde(default)]
    pub autostart: bool,
    /// 应用启动时自动拉起 Ollama 引擎（仅在引擎未运行时才启动，运行中则不打扰）
    #[serde(default)]
    pub start_engine_on_launch: bool,
    /// 完全退出应用时同时停止 Ollama 引擎（关闭 = 引擎继续后台运行）
    #[serde(default)]
    pub stop_engine_on_exit: bool,
}

impl Settings {
    pub fn load() -> Self {
        let path = Self::path();
        if let Ok(s) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<Settings>(&s) {
                return v;
            }
        }
        Settings {
            base_url: "http://localhost:11434".into(),
            theme: "system".into(),
            close_to_tray: true,
            refresh_interval: 8,
            ..Default::default()
        }
    }

    /// 写盘；返回是否成功。失败时调用方应给出反馈，而非静默丢弃（v1.0.7）。
    pub fn save(&self) -> bool {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string_pretty(self) {
            Ok(s) => std::fs::write(&path, s).is_ok(),
            Err(_) => false,
        }
    }

    pub fn path() -> PathBuf {
        let dir = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(dir)
            .join(".config")
            .join("offideskollama")
            .join("settings.json")
    }

    /// 仅用于「是否首次运行」判断（settings.json 不存在即首次）
    pub fn load_path() -> PathBuf {
        Self::path()
    }
}

/// 地址规范化：去空白、全角冒号转半角、无协议时自动补 `http://`、去尾部 `/`、
/// 剥离子路径（Ollama API 挂根路径，不支持反代子路径前缀）、纯端口数字按 localhost 处理。
/// 用户写 `192.168.1.5:11434` / `192.168.1.5：11434` / `http://h/ollama/` / `11434` 都能正确使用。
/// 所有自动修正都会回填输入框让用户可见（settings 保存路径），不悄悄改。
pub fn normalize_base_url(input: &str) -> String {
    let mut u = input.trim().replace('：', ":");
    if u.is_empty() {
        return "http://localhost:11434".to_string();
    }
    // 纯端口数字（如 11434）：全数字不是合法主机名，按 localhost:<port> 处理
    if !u.contains(':') && !u.contains('.') && u.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(port) = u.parse::<u16>() {
            if port > 0 {
                return format!("http://localhost:{u}");
            }
        }
    }
    if !u.starts_with("http://") && !u.starts_with("https://") {
        u = format!("http://{u}");
    }
    // 只写了协议没写主机（如 "http://"）→ 回落默认
    if u == "http://" || u == "https://" {
        return format!("{u}localhost:11434");
    }
    while u.ends_with('/') {
        u.pop();
    }
    // 剥离子路径：Ollama 不支持 base path，`http://h:11434/ollama` 一律取主机部分
    let scheme_end = u.find("://").map(|i| i + 3).unwrap_or(0);
    if let Some(i) = u[scheme_end..].find('/') {
        u.truncate(scheme_end + i);
    }
    u
}

/// 当前生效的 Ollama 服务地址。
/// 每次调用都读最新配置，因此用户在设置里改地址后无需重启即生效。
pub fn base_url() -> String {
    let s = Settings::load();
    normalize_base_url(&s.base_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_adds_scheme_and_trims() {
        assert_eq!(normalize_base_url("192.168.1.5:11434"), "http://192.168.1.5:11434");
        assert_eq!(normalize_base_url("  http://a.b:80/  "), "http://a.b:80");
        assert_eq!(normalize_base_url("https://x.y///"), "https://x.y");
        assert_eq!(normalize_base_url(""), "http://localhost:11434");
        assert_eq!(normalize_base_url("myhost"), "http://myhost");
        // 已带协议的不二次补
        assert_eq!(normalize_base_url("http://a:1"), "http://a:1");
    }

    #[test]
    fn normalize_edge_cases_auto_fixed() {
        // v1.0.8：以下输入由「记录缺口」升级为「自动修正 + 回填可见」
        assert_eq!(normalize_base_url("192.168.1.5：11434"), "http://192.168.1.5:11434"); // 全角冒号
        assert_eq!(normalize_base_url("http://h:11434/ollama/"), "http://h:11434"); // 子路径剥离
        assert_eq!(normalize_base_url("11434"), "http://localhost:11434"); // 纯端口
        assert_eq!(normalize_base_url("8080"), "http://localhost:8080"); // 纯端口（自定义）
        assert_eq!(normalize_base_url("http://"), "http://localhost:11434"); // 只写协议
        // https 前缀保留（不擅自降级——误写时靠测试连接的协议提示）
        assert_eq!(normalize_base_url("https://192.168.1.5:11434"), "https://192.168.1.5:11434");
        // 括号 IPv6 原样保留（无路径可剥）
        assert_eq!(normalize_base_url("http://[::1]:11434/"), "http://[::1]:11434");
        // 普通主机名不受纯端口规则影响
        assert_eq!(normalize_base_url("myhost"), "http://myhost");
        // 带点但非 IP 的输入不触发纯端口规则
        assert_eq!(normalize_base_url("a.b"), "http://a.b");
    }

    #[test]
    fn server_config_defaults_empty() {
        let s = Settings::default();
        assert_eq!(s.ollama_models_dir, "");
        assert_eq!(s.default_keep_alive, "");
        assert_eq!(s.num_parallel, 0);
        assert_eq!(s.max_loaded_models, 0);
        assert_eq!(s.gpu_visible, "");
    }

    #[test]
    fn server_config_round_trips() {
        let mut s = Settings::default();
        s.ollama_models_dir = "/data/ollama".into();
        s.default_keep_alive = "5m".into();
        s.num_parallel = 4;
        s.max_loaded_models = 2;
        s.gpu_visible = "0,1".into();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.ollama_models_dir, "/data/ollama");
        assert_eq!(back.default_keep_alive, "5m");
        assert_eq!(back.num_parallel, 4);
        assert_eq!(back.max_loaded_models, 2);
        assert_eq!(back.gpu_visible, "0,1");
    }

    #[test]
    fn server_config_missing_fields_load_as_default() {
        // 旧版设置（无服务端配置字段）反序列化时回落为默认空值，不应报错
        let json = r#"{"base_url":"http://localhost:11434","theme":"dark","close_to_tray":true,"refresh_interval":8}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.ollama_models_dir, "");
        assert_eq!(s.num_parallel, 0);
    }
}
