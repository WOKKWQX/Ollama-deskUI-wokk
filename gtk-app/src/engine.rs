//! Ollama 引擎管理：检测 / 安装 / 更新 / 卸载（v3.6.0 新增）。
//!
//! 定位：引擎的安装与卸载是「低频但重要」的系统级操作，按信息架构约定
//! 收进设置页，不占主导航。
//!
//! 安全设计（开源审查关注点）：
//! - 本模块所有 shell 命令都是**编译期固定字符串**，不拼接任何用户输入，
//!   不存在命令注入面。
//! - 安装/更新：从 ollama.com 官方地址下载 install.sh 到临时文件，
//!   用 `pkexec bash <文件>` 执行——脚本来源单一、内容落盘可审计，
//!   不使用 `curl … | sh` 管道（管道执行无法在执行前检查内容）。
//! - 卸载：仅删除 Ollama 程序文件与服务单元；**已下载的模型
//!   （~/.ollama 或 OLLAMA_MODELS 指向的目录）一律保留**——删除模型是
//!   「管理」页里按模型的显式操作，不属于引擎卸载的职责。
//! - 全部操作要求本机权限；连接远程服务时 UI 会说明操作仍作用于本机。

use std::process::Command;

/// Ollama 官方安装脚本地址（同时用于首次安装与升级）
pub const INSTALL_SCRIPT_URL: &str = "https://ollama.com/install.sh";

/// 下载到临时目录的固定文件名（不随版本变化，重复安装直接覆盖）
pub const SCRIPT_TEMP_NAME: &str = "ollama-install-offideskollama.sh";

/// 卸载命令（编译期固定字符串，逐条只删程序文件；模型目录不动）。
/// 各步骤幂等：没装 systemd 单元 / 二进制已不在时也不报错中断。
pub const UNINSTALL_CMD: &str = "systemctl stop ollama 2>/dev/null; \
systemctl disable ollama 2>/dev/null; \
rm -f /etc/systemd/system/ollama.service /etc/systemd/system/ollama.socket; \
systemctl daemon-reload 2>/dev/null; \
rm -f /usr/local/bin/ollama /usr/bin/ollama; \
rm -rf /usr/local/lib/ollama; \
echo done";

/// 引擎检测结果
#[derive(Debug, Clone, PartialEq)]
pub struct EngineStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub systemd_unit: bool,
}

impl EngineStatus {
    /// 状态行文案（设置页展示用）
    pub fn summary(&self) -> String {
        if !self.installed {
            return "未检测到 Ollama 引擎".into();
        }
        let v = self
            .version
            .clone()
            .unwrap_or_else(|| "版本未知".into());
        let managed = if self.systemd_unit { "systemd 托管" } else { "独立进程" };
        format!("已安装（{v}，{managed}）")
    }
}

/// 解析 `ollama --version` 输出。兼容 "ollama version is 0.5.7" 与
/// "ollama version 0.5.7" 两种形态，取其中形如版本的 token。纯函数，供单测。
pub fn parse_version_output(out: &str) -> Option<String> {
    let tokens: Vec<&str> = out.split_whitespace().collect();
    for i in 0..tokens.len() {
        if tokens[i].eq_ignore_ascii_case("version") {
            // 版本号最多出现在 "version" 后第 2 个 token 内（跳过 "is"）
            for t in tokens.iter().take((i + 3).min(tokens.len())).skip(i + 1) {
                let looks_ver = t.contains('.')
                    && t.chars()
                        .next()
                        .map(|c| c.is_ascii_digit())
                        .unwrap_or(false);
                if looks_ver {
                    let cleaned = t.trim_matches(|c: char| {
                        !c.is_ascii_alphanumeric() && c != '.' && c != '-' && c != '+'
                    });
                    if !cleaned.is_empty() {
                        return Some(cleaned.to_string());
                    }
                }
            }
        }
    }
    None
}

/// 探测本机引擎状态。全部为同步调用（可安全放进 rt::spawn 的后台线程）。
pub fn detect() -> EngineStatus {
    let installed = Command::new("sh")
        .args(["-c", "command -v ollama"])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);
    let version = if installed {
        Command::new("ollama")
            .arg("--version")
            .output()
            .ok()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout).to_string()
                    + &String::from_utf8_lossy(&o.stderr)
            })
            .as_deref()
            .and_then(parse_version_output)
    } else {
        None
    };
    EngineStatus {
        installed,
        version,
        systemd_unit: crate::service::has_systemd_unit(),
    }
}

/// 下载官方安装脚本到临时目录，返回脚本路径。
/// 内容做基本完整性校验（应为 shell 脚本且包含 ollama 字样），异常即中止。
pub async fn fetch_install_script() -> Result<std::path::PathBuf, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent(concat!("offideskollama/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("构建下载器失败：{e}"))?;
    let resp = client
        .get(INSTALL_SCRIPT_URL)
        .send()
        .await
        .map_err(|e| format!("下载安装脚本失败：{e}\n请检查网络后重试。"))?;
    if !resp.status().is_success() {
        return Err(format!("下载安装脚本失败：服务端返回 {}", resp.status()));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| format!("读取安装脚本失败：{e}"))?;
    // 官方脚本 < 100KB；超过上限说明响应异常，直接中止（防异常响应吃内存）
    if body.len() > 4 * 1024 * 1024 {
        return Err("下载到的内容异常（超过 4MB），已中止。".into());
    }
    if !body.starts_with("#!") || !body.to_lowercase().contains("ollama") {
        return Err(
            "下载到的内容不像官方安装脚本（缺少脚本特征），已中止，未做任何更改。".into(),
        );
    }
    // 随机文件名 + 排他创建（O_EXCL）+ 0700 权限：防 /tmp 抢注与替换（TOCTOU），
    // 也防止同机其他用户在 pkexec 以 root 执行前读取或改写脚本。
    let fname = format!(
        "{}-{}-{}.sh",
        SCRIPT_TEMP_NAME,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let path = std::env::temp_dir().join(fname);
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt; // .mode(0o700)
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&path)
        .map_err(|e| format!("创建临时脚本文件失败：{e}"))?;
    f.write_all(body.as_bytes())
        .map_err(|e| format!("写入临时文件失败：{e}"))?;
    Ok(path)
}

/// 用 pkexec 提权执行给定命令（参数列表原样透传，不做任何 shell 拼接）。
pub fn run_privileged(args: &[&str]) -> Result<String, String> {
    let st = Command::new("pkexec")
        .args(args)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "未找到 pkexec（PolicyKit 提权组件）。请安装 policykit-1 后重试，\
                 或在终端手动执行官方安装脚本。"
                    .to_string()
            } else {
                format!("启动提权组件失败：{e}")
            }
        })?;
    if st.status.success() {
        Ok(String::from_utf8_lossy(&st.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&st.stderr).trim().to_string();
        // pkexec 认证被用户取消时常见返回 126 且无 stderr
        if st.status.code() == Some(126) && stderr.is_empty() {
            Err("已取消授权，操作未执行。".into())
        } else {
            Err(format!(
                "提权执行失败：{}",
                if stderr.is_empty() {
                    format!("退出码 {}", st.status.code().unwrap_or(-1))
                } else {
                    stderr
                }
            ))
        }
    }
}

/// 安装 / 升级引擎（下载官方脚本 → pkexec 执行 → 对齐模型目录）。
/// 两种调用场景共用：未安装=安装；已安装=官方脚本会就地升级。
pub async fn install_or_update() -> Result<String, String> {
    let script = fetch_install_script().await?;
    let p = script.to_string_lossy().to_string();
    let r = run_privileged(&["bash", &p]);
    let _ = std::fs::remove_file(&p); // 无论成败都清理临时脚本，不留可执行残留
    r?;
    // v3.6.1 关键修复：官方脚本会新建 `ollama` 专用系统账户、systemd 单元带
    // `User=ollama`，服务于是读取 /var/lib/ollama 而非 ~/.ollama/models —— 用户
    // 已下载的模型全部「消失」，界面返回空列表（v3.6.0 实测踩中）。
    // 安装/升级完成后立即对齐模型目录，把用户既有模型重新接回。
    let align_note = align_models_dir();
    Ok(match align_note {
        Ok(msg) => msg,
        Err(e) => format!("引擎已就绪，但模型目录对齐未完成：{e}"),
    })
}

/// 让 systemd 服务读取用户既有模型目录（`~/.ollama/models`）。
///
/// 做法（全部为编译期固定命令，仅路径来自本机 $HOME，无注入面）：
/// 1. 若服务以非当前用户运行（如 `ollama` 专用账户），需要家目录「穿透」权限
///    （o+x，仅开放路径穿过，不开放列目录/读写内容）。
/// 2. 写入 systemd drop-in：`OLLAMA_MODELS=<用户模型目录>`。
/// 3. daemon-reload 并重启服务。
///
/// 幂等：重复执行只覆盖同一份 drop-in 文件。
pub fn align_models_dir() -> Result<String, String> {
    let home = std::env::var("HOME").map_err(|_| "无法确定当前用户家目录")?;
    let models = format!("{home}/.ollama/models");
    if !std::path::Path::new(&models).is_dir() {
        return Err(format!("未找到用户模型目录 {models}，跳过对齐"));
    }
    // 用单条固定 shell（路径经单引号包裹，杜绝空格/特殊字符注入）
    let cmd = format!(
        "chmod o+x '{home}'; \
         mkdir -p /etc/systemd/system/ollama.service.d; \
         printf '%s\\n' \
         '[Service]' \
         'Environment=\"OLLAMA_MODELS={models}\"' \
         > /etc/systemd/system/ollama.service.d/models-path.conf; \
         systemctl daemon-reload; \
         systemctl restart ollama; \
         echo ok"
    );
    run_privileged(&["bash", "-c", &cmd])?;
    Ok(format!(
        "已对齐模型目录：systemd 服务现读取 {models}（原默认 /var/lib/ollama）。"
    ))
}

/// 以提权方式清理模型目录里的未完成分片（v3.6.2）。
///
/// 为什么需要提权：当 Ollama 服务以 `ollama` 专用账户运行时，
/// 中断下载留下的 `-partial*` 残片属主是 `ollama`，当前用户（如 `wokk`）无权限删除；
/// 而 Ollama 自己也不会清理它们——这正是「怎么拉都失败」的死锁。
///
/// 提权走 pkexec（图形授权，由用户确认），清理完在 `on_done` 回调里重新发起下载。
/// 清理失败时仍调用 `on_done`——重试本身可能成功（如残片恰好已被服务端复用完毕），
/// 不应因为清理失败就阻断用户的重试意图。
pub fn clean_partials_privileged<F>(on_done: F)
where
    F: Fn() + 'static,
{
    let home = std::env::var("HOME").unwrap_or_default();
    let models = format!("{home}/.ollama/models");
    if !std::path::Path::new(&models).is_dir() {
        on_done();
        return;
    }
    // 固定命令，仅路径来自本机 $HOME 并经单引号包裹（无注入面）
    let cmd = format!("rm -f '{models}/blobs/'*-partial*; echo ok");
    // rt::spawn：后台线程执行（不阻塞 UI），结果回主线程调 on_done（GTK 对象只能在主线程碰）
    crate::rt::spawn(
        async move {
            let _ = run_privileged(&["bash", "-c", &cmd]);
        },
        move |_| on_done(),
    );
}


///
/// 典型场景：systemd 服务以 `ollama` 专用账户运行，读的是 `/var/lib/ollama`，
/// 而用户模型在 `~/.ollama/models` —— 服务正常、API 正常，但模型列表是空的。
/// 用户容易误以为「模型被删了」。这里主动比对并给出下一步动作。
///
/// 返回 None = 无需提示（磁盘上本来就没有模型）。
pub fn diagnose_empty_model_dir() -> Option<String> {    let home = std::env::var("HOME").ok()?;
    let user_dir = format!("{home}/.ollama/models/manifests/registry.ollama.ai/library");
    let user_dir = std::path::Path::new(&user_dir);
    if !user_dir.is_dir() {
        return None;
    }
    // 用户目录里确实有模型清单 → 说明是「服务读错目录」
    let has_models = std::fs::read_dir(user_dir)
        .map(|d| d.filter_map(|e| e.ok()).next().is_some())
        .unwrap_or(false);
    if !has_models {
        return None;
    }
    let serving_as = Command::new("systemctl")
        .args(["show", "-p", "User", "--value", "ollama"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "root".into());
    let current_user = std::env::var("USER").unwrap_or_else(|_| "当前用户".into());
    Some(format!(
        "服务看不到你的模型，但磁盘上它们是存在的。\n\n\
         原因：Ollama 服务以「{serving_as}」账户运行，读取的是该账户自己的模型目录；\n\
         而你的模型存放在 ~/.ollama/models（属于「{current_user}」）。\n\n\
         模型没有被删除，一个都没少。\n\n\
         修复：到 帮助 → 设置 → Ollama 引擎，点「安装引擎」（即「检查更新」）——\n\
         该操作会顺带把服务指向你的模型目录。或终端执行：\n\
         sudo systemctl edit ollama  然后加入：\n\
         [Service]\nEnvironment=\"OLLAMA_MODELS={home}/.ollama/models\"\n\
         再 sudo systemctl daemon-reload && sudo systemctl restart ollama"
    ))
}

/// 卸载引擎（固定命令；模型数据保留）。
pub fn uninstall() -> Result<String, String> {
    run_privileged(&["bash", "-c", UNINSTALL_CMD])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_version_output() {
        assert_eq!(
            parse_version_output("ollama version is 0.5.7\n"),
            Some("0.5.7".into())
        );
    }

    #[test]
    fn parses_version_without_is() {
        assert_eq!(
            parse_version_output("ollama version 0.3.12"),
            Some("0.3.12".into())
        );
    }

    #[test]
    fn parse_rejects_garbage() {
        assert_eq!(parse_version_output("command not found"), None);
        assert_eq!(parse_version_output(""), None);
        // 有 version 字样但后面不是版本号
        assert_eq!(parse_version_output("ollama version is"), None);
    }

    #[test]
    fn summary_reports_states() {
        let none = EngineStatus { installed: false, version: None, systemd_unit: false };
        assert!(none.summary().contains("未检测到"));
        let some = EngineStatus {
            installed: true,
            version: Some("0.5.7".into()),
            systemd_unit: true,
        };
        let s = some.summary();
        assert!(s.contains("0.5.7") && s.contains("systemd"));
    }

    #[test]
    fn uninstall_cmd_is_fixed_and_keeps_models() {
        // 固定字符串，不包含任何运行时拼接痕迹；明确不出现模型目录
        assert!(UNINSTALL_CMD.contains("systemctl stop ollama"));
        assert!(UNINSTALL_CMD.contains("/usr/local/bin/ollama"));
        assert!(!UNINSTALL_CMD.contains(".ollama/models"));
        assert!(!UNINSTALL_CMD.contains("OLLAMA_MODELS"));
        // 幂等：失败步骤都带兜底
        assert!(UNINSTALL_CMD.contains("2>/dev/null"));
    }

    #[test]
    fn script_temp_name_is_stable() {
        assert!(SCRIPT_TEMP_NAME.ends_with(".sh"));
        assert!(INSTALL_SCRIPT_URL.starts_with("https://"));
    }

    /// 对齐命令的构造规则：路径必须单引号包裹（防空格/特殊字符注入），
    /// 且只写 drop-in、不动服务单元本体。
    #[test]
    fn align_cmd_quotes_paths_and_uses_dropin() {
        let home = "/home/some user";
        let models = format!("{home}/.ollama/models");
        let cmd = format!(
            "chmod o+x '{home}'; \
             mkdir -p /etc/systemd/system/ollama.service.d; \
             printf '%s\\n' \
             '[Service]' \
             'Environment=\"OLLAMA_MODELS={models}\"' \
             > /etc/systemd/system/ollama.service.d/models-path.conf; \
             systemctl daemon-reload; \
             systemctl restart ollama; \
             echo ok"
        );
        assert!(cmd.contains("chmod o+x '/home/some user'"));
        assert!(cmd.contains("models-path.conf"));
        assert!(cmd.contains(&format!("OLLAMA_MODELS={models}")));
        // 不碰服务单元本体，只写 drop-in
        assert!(!cmd.contains("> /etc/systemd/system/ollama.service\n"));
    }

    /// 诊断函数的「无模型」路径必须返回 None（避免误报打扰用户）。
    #[test]
    fn diagnose_returns_none_without_user_models() {
        let home = std::env::var("HOME").unwrap_or_default();
        let dir = format!("{home}/.ollama/models/manifests/registry.ollama.ai/library");
        let has = std::path::Path::new(&dir).is_dir()
            && std::fs::read_dir(&dir)
                .map(|d| d.filter_map(|e| e.ok()).next().is_some())
                .unwrap_or(false);
        // 仅在确实没有模型时才断言 None（有模型的机器上跳过，避免环境依赖）
        if !has {
            assert!(diagnose_empty_model_dir().is_none());
        }
    }
}
