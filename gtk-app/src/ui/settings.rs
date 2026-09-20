//! 设置对话框 + 更新日志板
//! 设置项：Ollama 服务地址、主题模式、关闭到托盘、自动刷新间隔、清除会话。
//! 持久化到 ~/.config/offideskollama/settings.json
//! Settings 结构定义见 crate::config（ui 与 ollama 共享，避免循环依赖）。
//!
//! v3.4.0 设置页原生化重设计：
//! - 按 GNOME / Cinnamon 系统设置惯例，把零散设置项分组为 .frame 卡片；
//! - 行内控件按「标题在左、操作在右」排布，开关/动作行近似 rich-list；
//! - 继续使用主题确实定义的类（.frame / .toolbar / .linked / .title-4 / .heading /
//!   .caption / .dim-label），不引入 .card/.boxed-list/.preferences 等无效类。

use gtk4::prelude::*;

pub use crate::config::Settings;

/// 根据设置应用主题模式（跟随系统 / 强制深 / 强制浅）。
/// 纯 GTK4 无 libadwaita 的 StyleManager。v3.0.2 修复：
/// 原实现只拨 gtk-application-prefer-dark-theme，该开关要求主题存在
/// "<名称>-dark" 变体；而 Mint-Y 家族的深色版是独立主题目录
/// （Mint-Y-Aqua → Mint-Y-Dark-Aqua），变体缺失导致三档切换全部无效。
/// 现改为：深/浅档把本应用的主题名切到对应变体目录（仅本进程生效，
/// 不写系统设置），「跟随系统」还原本机主题继续由 XSettings 驱动。
pub fn apply_theme() {
    let settings = match gtk4::Settings::default() {
        Some(s) => s,
        None => return,
    };
    let sys = system_theme_name().to_string();
    match Settings::load().theme.as_str() {
        "dark" => {
            let dark = dark_variant(&sys);
            if dark != sys {
                settings.set_gtk_theme_name(Some(&dark));
            }
            settings.set_gtk_application_prefer_dark_theme(true);
        }
        "light" => {
            let light = light_variant(&sys);
            if light != sys {
                settings.set_gtk_theme_name(Some(&light));
            }
            settings.set_gtk_application_prefer_dark_theme(false);
        }
        // "system"：还原本机主题，交由系统（XSettings）驱动，桌面换主题实时跟随
        _ => {
            settings.set_gtk_theme_name(Some(&sys));
            settings.set_gtk_application_prefer_dark_theme(false);
        }
    }
}

/// 应用启动那一刻的系统主题名（首次调用时捕获；此后即使被深/浅档覆盖，
/// 「跟随系统」仍能还原到它）。启动时机在窗口构建之前，读到的是原始值。
fn system_theme_name() -> &'static str {
    use std::sync::OnceLock;
    static SYS: OnceLock<String> = OnceLock::new();
    SYS.get_or_init(|| {
        gtk4::Settings::default()
            .and_then(|s| s.gtk_theme_name())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Mint-Y".to_string())
    })
    .as_str()
}

fn theme_dir_exists(name: &str) -> bool {
    std::path::Path::new("/usr/share/themes").join(name).is_dir()
}

/// 求深色变体：Mint-Y-Aqua → Mint-Y-Dark-Aqua，Mint-Y → Mint-Y-Dark；
/// 其他主题先试 "<名称>-dark"（覆盖 Adwaita 等自带变体的主题）；已含 Dark 原样返回。
fn dark_variant(name: &str) -> String {
    if name.contains("Dark") {
        return name.to_string();
    }
    if let Some(rest) = name.strip_prefix("Mint-Y-") {
        let cand = format!("Mint-Y-Dark-{}", rest);
        if theme_dir_exists(&cand) {
            return cand;
        }
    }
    if name == "Mint-Y" && theme_dir_exists("Mint-Y-Dark") {
        return "Mint-Y-Dark".to_string();
    }
    let cand = format!("{}-dark", name);
    if theme_dir_exists(&cand) {
        return cand;
    }
    name.to_string()
}

/// 求浅色变体：Mint-Y-Dark-Aqua → Mint-Y-Aqua，Mint-Y-Dark → Mint-Y，
/// "<名称>-dark" → "<名称>"；本就浅色则原样返回。
fn light_variant(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("Mint-Y-Dark-") {
        let cand = format!("Mint-Y-{}", rest);
        if theme_dir_exists(&cand) {
            return cand;
        }
    }
    if name == "Mint-Y-Dark" && theme_dir_exists("Mint-Y") {
        return "Mint-Y".to_string();
    }
    if let Some(rest) = name.strip_suffix("-dark") {
        if theme_dir_exists(rest) {
            return rest.to_string();
        }
    }
    name.to_string()
}

// ============================================================================
// 原生设置页控件工厂（v3.4.0）
// ============================================================================

/// 设置分组卡片：外层 `.frame` 给边框，内层垂直 Box 统一留白。
/// 返回 (frame, inner)，控件都加到 inner 里。
fn pref_group(title: &str) -> (gtk4::Box, gtk4::Box) {
    let frame = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .css_classes(vec!["frame"])
        .build();
    let inner = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    let lbl = gtk4::Label::builder()
        .label(title)
        .halign(gtk4::Align::Start)
        .css_classes(vec!["title-4", "heading"])
        .build();
    inner.append(&lbl);
    frame.append(&inner);
    (frame, inner)
}

/// 分组顶部说明文字（dim-label + caption）。
fn pref_hint(text: &str) -> gtk4::Label {
    gtk4::Label::builder()
        .label(text)
        .halign(gtk4::Align::Start)
        .wrap(true)
        .css_classes(vec!["caption", "dim-label"])
        .build()
}

/// 开关行：左侧标题+可选副标题（左撑满），右侧开关。
fn pref_switch_row(title: &str, subtitle: Option<&str>, sw: &gtk4::Switch) -> gtk4::Box {
    let row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .build();
    let text = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    let title_lbl = gtk4::Label::builder()
        .label(title)
        .halign(gtk4::Align::Start)
        .build();
    text.append(&title_lbl);
    if let Some(sub) = subtitle {
        let sub_lbl = gtk4::Label::builder()
            .label(sub)
            .halign(gtk4::Align::Start)
            .wrap(true)
            .css_classes(vec!["caption", "dim-label"])
            .build();
        text.append(&sub_lbl);
    }
    sw.set_valign(gtk4::Align::Center);
    row.append(&text);
    row.append(sw);
    row
}

/// 动作行：左侧标题+可选副标题（左撑满），右侧任意控件（按钮、下拉框等）。
fn pref_action_row<W: IsA<gtk4::Widget>>(title: &str, subtitle: Option<&str>, control: &W) -> gtk4::Box {
    let row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .build();
    let text = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    let title_lbl = gtk4::Label::builder()
        .label(title)
        .halign(gtk4::Align::Start)
        .build();
    text.append(&title_lbl);
    if let Some(sub) = subtitle {
        let sub_lbl = gtk4::Label::builder()
            .label(sub)
            .halign(gtk4::Align::Start)
            .wrap(true)
            .css_classes(vec!["caption", "dim-label"])
            .build();
        text.append(&sub_lbl);
    }
    control.set_valign(gtk4::Align::Center);
    row.append(&text);
    row.append(control);
    row
}

/// 输入项：标题 + 可选副标题 + 输入控件（Entry/SpinButton 等），控件横向撑满。
fn pref_field_row<W: IsA<gtk4::Widget>>(title: &str, subtitle: Option<&str>, widget: &W) -> gtk4::Box {
    let col = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(6)
        .build();
    let title_lbl = gtk4::Label::builder()
        .label(title)
        .halign(gtk4::Align::Start)
        .build();
    col.append(&title_lbl);
    if let Some(sub) = subtitle {
        let sub_lbl = gtk4::Label::builder()
            .label(sub)
            .halign(gtk4::Align::Start)
            .wrap(true)
            .css_classes(vec!["caption", "dim-label"])
            .build();
        col.append(&sub_lbl);
    }
    widget.set_halign(gtk4::Align::Fill);
    widget.set_hexpand(true);
    col.append(widget);
    col
}

// ============================================================================

/// 首次使用引导（v1.0.22 重做）：基于通用分步向导框架，内容对齐当前界面。
/// 旧版用「上一步/下一步 + 纯文字」，且仍在描述 1.0.16 之前已删除的界面
/// （左侧模型列表 / 聊天·生成·已加载·嵌入 / 顶栏启停），属于教错用户。
pub fn open_welcome(win: &gtk4::Window) {
    use crate::ui::stepper::{page_box, StepWizard};

    let w = StepWizard::new(win, "首次使用引导", 620, 520);

    w.add_page(
        "欢迎",
        "本应用是本机 Ollama 的图形化管理工具，不是在线服务。",
        &page_box(&[
            "O-deskUI .wokk. 管理你自己机器上的 Ollama：模型都在本地，对话、生成、嵌入全部在你本机完成，本应用不提供任何云端能力，也不上传你的数据。",
            "整个界面只有两处入口：左侧四栏导航，和顶部菜单栏。下面的引导只讲你会用到的部分，大约一分钟。",
        ]),
        None,
    );

    w.add_page(
        "连上本机服务",
        "先确认 Ollama 服务在跑。",
        &page_box(&[
            "应用通过 HTTP 连接本机 Ollama（默认 http://localhost:11434）。打开后看「概览」页顶部的状态灯：显示 Ollama 版本号即为已连接，显示「未连接」说明服务没起来。",
            "服务未运行时，用「概览」页的启动按钮，或顶部菜单栏「操作 → 打开 Ollama 引擎」启动它。",
            "想让它开机就位：到「帮助 → 设置」，打开「开机自启动」与「应用启动时自动运行 Ollama 引擎」，以后开机进系统即可直接用。",
            "本机还没装 Ollama？到「帮助 → 设置 → Ollama 引擎」点「安装引擎」：会下载官方脚本并请求管理员授权，已装的模型不受影响。",
        ]),
        None,
    );

    w.add_page(
        "界面导览",
        "左侧四栏 + 顶部菜单栏。",
        &page_box(&[
            "概览：服务状态、Ollama 版本、已加载模型与显存占用，以及服务启停。",
            "管理：本地模型的主战场——查看、复制、删除已装模型。",
            "云库：浏览与搜索 Ollama 官方模型库，按你的硬件标「流畅 / 勉强 / 放不下」，直接拉取。",
            "测试：对话 / 生成 / 嵌入 三个页内切换，用来试用模型。",
            "顶部菜单栏：查看（主题、本机信息与建议参数）· 转到（主页、模型管理）· 操作（引擎开关、关闭所有运行中的模型）· 帮助（设置、服务日志、注册中心）。",
        ]),
        None,
    );

    w.add_page(
        "装第一个模型",
        "云库选，点拉取。",
        &page_box(&[
            "到「云库」页：内置 120 项精选模型目录（按用途分类、每条附一句话简介），也可搜索官方库全部模型。每张卡片会按你的显存标出「流畅 / 勉强 / 放不下」，不确定就挑标「流畅」的。",
            "点「拉取」开始下载。下载完成后，模型出现在「管理」页。",
            "「管理」页可以复制模型名、查看详情、删除不再需要的模型。",
        ]),
        None,
    );

    w.add_page(
        "跑起来",
        "测试页选模型即用。",
        &page_box(&[
            "到「测试」页，顶部切换 对话 / 生成 / 嵌入。",
            "对话页顶部有模型下拉，选一个已装模型就能直接开始聊，不必先去别处选。",
            "生成页用来做文本补全，嵌入页用来取向量，两页同样可在页内选择模型。",
            "已加载的模型会占用显存，用「操作 → 关闭所有正在运行的模型」可以一次释放。",
        ]),
        None,
    );

    let last = page_box(&[
        "「帮助 → 设置」里可以调：服务地址、主题（跟随系统 / 深 / 浅）、关闭窗口时最小化到托盘。",
        "引擎本身也能在设置里安装、升级、卸载（「Ollama 引擎」一栏）；卸载只删引擎，已下载的模型全部保留。",
        "底部「启动与退出」三项：开机自启动（静默进托盘不弹窗）、应用启动时自动运行 Ollama 引擎、退出应用时同时关闭 Ollama 引擎。",
        "想系统学习各功能怎么用：教学文档按场景整理了完整教学（连接 / 装模型 / 跑模型 / 参数 / 排障），在「帮助 → 设置」里随时打开。",
    ]);
    let docs_btn = gtk4::Button::builder()
        .label("打开教学文档")
        .halign(gtk4::Align::Start)
        .margin_top(8)
        .build();
    {
        let dw = win.clone();
        docs_btn.connect_clicked(move |_| {
            open_docs(&dw);
        });
    }
    last.append(&docs_btn);
    w.add_page(
        "完成",
        "常用设置与教学文档。",
        &last,
        None,
    );

    // 关闭即视为已看过（写一次配置文件；失败仅意味着下次仍会看到，可容忍）
    {
        let wd = w.window();
        wd.connect_close_request(move |_| {
            let s = Settings::load();
            let _ = s.save();
            glib::Propagation::Proceed
        });
    }
    w.on_finish("开始使用", || None);
    w.present();
}

/// 打开设置对话框
pub fn open_settings(win: &gtk4::Window) {
    let content = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(16)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();

    // ------------------------------------------------------------------------
    // 连接
    // ------------------------------------------------------------------------
    let (conn_card, conn_inner) = pref_group("连接");
    conn_inner.append(&pref_hint("应用通过 HTTP 连接本机 Ollama。修改地址后先点「测试连接」验证，保存后即时生效。"));

    let base_entry = gtk4::Entry::builder()
        .text(&Settings::load().base_url)
        .placeholder_text("http://localhost:11434")
        .build();
    let test_btn = gtk4::Button::builder().label("测试连接").build();
    let base_row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .css_classes(vec!["linked"])
        .build();
    base_entry.set_hexpand(true);
    base_row.append(&base_entry);
    base_row.append(&test_btn);
    conn_inner.append(&base_row);

    // 记住打开时的地址（关闭时对比，变更才提示「已生效」，避免每次关窗都打扰）
    let prev_base = crate::config::normalize_base_url(&base_entry.text());
    {
        let entry = base_entry.clone();
        let dlg_win = win.clone();
        let btn = test_btn.clone();
        test_btn.connect_clicked(move |_| {
            let url = crate::config::normalize_base_url(&entry.text());
            btn.set_sensitive(false);
            btn.set_label("测试中…");
            let btn2 = btn.clone();
            let win_t = dlg_win.clone();
            crate::rt::spawn(
                async move {
                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(3))
                        .timeout(std::time::Duration::from_secs(5))
                        .build()
                        .expect("构建测试 client 失败");
                    let t0 = std::time::Instant::now();
                    let res = client
                        .get(format!("{url}/api/version"))
                        .send()
                        .await
                        .map(|r| r.status().is_success());
                    (url, res, t0.elapsed())
                },
                move |(url, ok, _)| {
                    btn2.set_sensitive(true);
                    btn2.set_label("测试连接");
                    if matches!(ok, Ok(true)) {
                        crate::ui::dialogs::info(&win_t, "连接成功", &format!("{url} 可正常访问。"));
                    } else {
                        // v1.0.7：失败原因分类给出，不再只显示同一份通用清单
                        let reason = connect_failure_reason(&ok);
                        let proto_hint = if url.starts_with("https://") {
                            "\n4. 地址用了 https://：若对端未启用 HTTPS，请改为 http://。"
                        } else {
                            ""
                        };
                        crate::ui::dialogs::show_err(&win_t, &format!(
                            "无法连接 {url}\n原因：{reason}\n\n请依次检查：\n\
                            1. 地址与端口是否写对（远程机器需写 IP:端口，如 192.168.1.5:11434）；\n\
                            2. 对端 Ollama 是否在运行，且 OLLAMA_HOST 已设为 0.0.0.0 才接受外部访问；\n\
                            3. 防火墙是否放行该端口。{proto_hint}"
                        ));
                    }
                },
            );
        });
    }
    content.append(&conn_card);

    // ------------------------------------------------------------------------
    // 外观
    // ------------------------------------------------------------------------
    let (theme_card, theme_inner) = pref_group("外观");
    theme_inner.append(&pref_hint("主题模式仅对本应用生效，切换后即时生效，不影响系统主题。"));

    let combo = gtk4::DropDown::from_strings(&["跟随系统", "强制深色", "强制浅色"]);
    {
        let s = Settings::load();
        combo.set_selected(match s.theme.as_str() {
            "dark" => 1,
            "light" => 2,
            _ => 0,
        });
    }
    theme_inner.append(&pref_action_row("主题模式", None, &combo));
    content.append(&theme_card);

    // ------------------------------------------------------------------------
    // 服务端配置
    // ------------------------------------------------------------------------
    let (srv_card, srv_inner) = pref_group("服务端配置");
    srv_inner.append(&pref_hint("以下环境变量在 Ollama 引擎启动时生效，修改后需重启引擎。"));

    let models_entry = gtk4::Entry::builder()
        .text(&Settings::load().ollama_models_dir)
        .placeholder_text("/usr/share/ollama/models")
        .build();
    srv_inner.append(&pref_field_row("模型目录 OLLAMA_MODELS", Some("空表示使用 Ollama 默认路径。"), &models_entry));

    let ka_entry = gtk4::Entry::builder()
        .text(&Settings::load().default_keep_alive)
        .placeholder_text("5m / -1")
        .build();
    srv_inner.append(&pref_field_row("默认常驻 keep_alive", Some("例如 5m、30m、-1（常驻）。"), &ka_entry));

    let np_adj = gtk4::Adjustment::new(Settings::load().num_parallel as f64, 0.0, 32.0, 1.0, 4.0, 0.0);
    let np_spin = gtk4::SpinButton::builder().adjustment(&np_adj).numeric(true).build();
    srv_inner.append(&pref_field_row("并发请求数 OLLAMA_NUM_PARALLEL", Some("0 表示使用 Ollama 默认值。"), &np_spin));

    let ml_adj = gtk4::Adjustment::new(Settings::load().max_loaded_models as f64, 0.0, 16.0, 1.0, 2.0, 0.0);
    let ml_spin = gtk4::SpinButton::builder().adjustment(&ml_adj).numeric(true).build();
    srv_inner.append(&pref_field_row("并行加载模型数 OLLAMA_MAX_LOADED_MODELS", Some("0 表示使用 Ollama 默认值。"), &ml_spin));

    let gpu_entry = gtk4::Entry::builder()
        .text(&Settings::load().gpu_visible)
        .placeholder_text("空=全部，如 0,1")
        .build();
    srv_inner.append(&pref_field_row("可见 GPU OLLAMA_GPU", Some("空表示对所有 GPU 可见。"), &gpu_entry));

    content.append(&srv_card);

    // ------------------------------------------------------------------------
    // 行为
    // ------------------------------------------------------------------------
    let (behav_card, behav_inner) = pref_group("行为");

    let adj = gtk4::Adjustment::new(Settings::load().refresh_interval.max(1) as f64, 1.0, 120.0, 1.0, 10.0, 0.0);
    let refresh_spin = gtk4::SpinButton::builder().adjustment(&adj).numeric(true).build();
    behav_inner.append(&pref_field_row("已加载模型自动刷新（秒）", Some("概览页与托盘状态按此间隔轮询。"), &refresh_spin));

    let tray_sw = gtk4::Switch::builder().active(Settings::load().close_to_tray).build();
    behav_inner.append(&pref_switch_row(
        "关闭窗口时最小化到托盘",
        Some("点击窗口关闭按钮后隐藏到系统托盘，而不是退出应用。"),
        &tray_sw,
    ));

    let auto_sw = gtk4::Switch::builder().active(Settings::load().autostart).build();
    behav_inner.append(&pref_switch_row(
        "开机自启动",
        Some("登录后静默启动到托盘，不弹出主窗口。"),
        &auto_sw,
    ));

    let eng_sw = gtk4::Switch::builder().active(Settings::load().start_engine_on_launch).build();
    behav_inner.append(&pref_switch_row(
        "应用启动时自动运行 Ollama 引擎",
        Some("仅在引擎未运行时拉起，已运行则不重复启动。"),
        &eng_sw,
    ));

    let exit_sw = gtk4::Switch::builder().active(Settings::load().stop_engine_on_exit).build();
    behav_inner.append(&pref_switch_row(
        "退出应用时同时关闭 Ollama 引擎",
        Some("关闭此项后，退出应用时引擎继续在后台运行。"),
        &exit_sw,
    ));

    content.append(&behav_card);

    // ------------------------------------------------------------------------
    // Ollama 引擎（v3.6.0 新增）：检测 / 安装 / 升级 / 卸载
    // ------------------------------------------------------------------------
    let (eng_card, eng_inner) = pref_group("Ollama 引擎");
    eng_inner.append(&pref_hint(
        "安装 / 升级 / 卸载会请求管理员授权（pkexec）。卸载只删除引擎本身，已下载的模型全部保留。",
    ));
    let eng_status_lbl = gtk4::Label::builder()
        .label("正在检测引擎…")
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    eng_inner.append(&eng_status_lbl);
    if !crate::service::is_local_target() {
        eng_inner.append(&pref_hint(
            "当前连接的是远程 Ollama 服务；这里的引擎操作仍然只作用于本机。",
        ));
    }
    let eng_btn_row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .build();
    let eng_install_btn = gtk4::Button::builder()
        .label("安装引擎")
        .css_classes(vec!["suggested-action"])
        .build();
    let eng_uninstall_btn = gtk4::Button::builder()
        .label("卸载引擎")
        .css_classes(vec!["destructive-action"])
        .build();
    eng_btn_row.append(&eng_install_btn);
    eng_btn_row.append(&eng_uninstall_btn);
    eng_inner.append(&eng_btn_row);
    content.append(&eng_card);

    // 打开设置时后台探测一次引擎状态（探测是同步命令，放后台线程防卡 UI）
    {
        let lbl = eng_status_lbl.clone();
        let ib = eng_install_btn.clone();
        let ub = eng_uninstall_btn.clone();
        crate::rt::spawn(
            async move { crate::engine::detect() },
            move |st: crate::engine::EngineStatus| {
                lbl.set_label(&st.summary());
                ib.set_label(if st.installed { "检查更新" } else { "安装引擎" });
                ub.set_visible(st.installed);
            },
        );
    }

    // 安装 / 检查更新（官方脚本幂等：未装=安装，已装=就地升级）
    {
        let w = win.clone();
        let st_lbl = eng_status_lbl.clone();
        let ib = eng_install_btn.clone();
        let ub = eng_uninstall_btn.clone();
        eng_install_btn.connect_clicked(move |_| {
            let updating = crate::engine::detect().installed;
            let (title, label) = if updating {
                ("升级 Ollama 引擎？", "检查更新并升级")
            } else {
                ("安装 Ollama 引擎？", "开始安装")
            };
            let body = format!(
                "将从 {}\n下载官方安装脚本，并请求管理员授权（pkexec）执行。\n\
                 需要网络连接，全程约 1–5 分钟；下载与授权期间可继续使用应用。",
                crate::engine::INSTALL_SCRIPT_URL
            );
            let w2 = w.clone();
            let st2 = st_lbl.clone();
            let ib2 = ib.clone();
            let ub2 = ub.clone();
            crate::ui::dialogs::confirm_primary(&w, title, &body, label, move || {
                // Fn 闭包会被多次调用：先克隆句柄，避免把捕获 move 进内层闭包
                let w2 = w2.clone();
                let st2 = st2.clone();
                let ib2 = ib2.clone();
                let ub2 = ub2.clone();
                // 提权命令无法安全中途终止，进度窗保持模态、不提供取消；
                // 直接关闭本窗不会中止后台操作，完成后状态行会自行刷新。
                let pd = crate::ui::dialogs::progress(&w2, "Ollama 引擎", false);
                pd.set_text(if updating {
                    "正在下载并升级引擎…（关闭本窗口不会中止操作）"
                } else {
                    "正在下载并安装引擎…（关闭本窗口不会中止操作）"
                });
                ib2.set_sensitive(false);
                ub2.set_sensitive(false);
                let pd2 = pd.clone();
                crate::rt::spawn(
                    async move { crate::engine::install_or_update().await },
                    move |res: Result<String, String>| {
                        pd2.close();
                        let st = crate::engine::detect();
                        st2.set_label(&st.summary());
                        ib2.set_label(if st.installed { "检查更新" } else { "安装引擎" });
                        ub2.set_visible(st.installed);
                        ub2.set_sensitive(true);
                        ib2.set_sensitive(true);
                        match res {
                            Ok(_) => crate::ui::dialogs::info(
                                &w2,
                                "完成",
                                "Ollama 引擎已就绪。\n官方脚本会自动启动系统服务；若未自动启动，可在概览页点「启动」。",
                            ),
                            Err(e) => crate::ui::dialogs::show_err(&w2, &e),
                        }
                    },
                );
            });
        });
    }

    // 卸载（破坏性操作：destructive 确认框；模型数据保留并明确告知）
    {
        let w = win.clone();
        let st_lbl = eng_status_lbl.clone();
        let ib = eng_install_btn.clone();
        let ub = eng_uninstall_btn.clone();
        eng_uninstall_btn.connect_clicked(move |_| {
            let w2 = w.clone();
            let st2 = st_lbl.clone();
            let ib2 = ib.clone();
            let ub2 = ub.clone();
            crate::ui::dialogs::confirm(
                &w,
                "卸载 Ollama 引擎？",
                "将停止 ollama 服务并删除引擎程序文件。\n已下载的模型全部保留在磁盘，不会被删除。\n\n操作需要管理员授权，确认后不可撤销。",
                "确认卸载",
                move || {
                    // Fn 闭包内先克隆句柄，避免把捕获 move 进内层闭包
                    let w2 = w2.clone();
                    let st2 = st2.clone();
                    let ib2 = ib2.clone();
                    let ub2 = ub2.clone();
                    let pd = crate::ui::dialogs::progress(&w2, "Ollama 引擎", false);
                    pd.set_text("正在卸载引擎…");
                    ib2.set_sensitive(false);
                    ub2.set_sensitive(false);
                    let pd2 = pd.clone();
                    crate::rt::spawn(
                        async move { crate::engine::uninstall() },
                        move |res: Result<String, String>| {
                            pd2.close();
                            let st = crate::engine::detect();
                            st2.set_label(&st.summary());
                            ib2.set_label(if st.installed { "检查更新" } else { "安装引擎" });
                            ub2.set_visible(st.installed);
                            ub2.set_sensitive(true);
                            ib2.set_sensitive(true);
                            match res {
                                Ok(_) => crate::ui::dialogs::info(
                                    &w2,
                                    "完成",
                                    "Ollama 引擎已卸载。\n已下载的模型仍保留在磁盘；重装引擎后可直接继续使用。",
                                ),
                                Err(e) => crate::ui::dialogs::show_err(&w2, &e),
                            }
                        },
                    );
                },
            );
        });
    }

    // ------------------------------------------------------------------------
    // 云库索引
    // ------------------------------------------------------------------------
    let (cat_card, cat_inner) = pref_group("云库索引");
    cat_inner.append(&pref_hint("内置模型清单随版本升级自动更新；你的增改以叠加方式保存，可单独恢复默认。"));

    let cat_btn = gtk4::Button::builder().label("编辑").build();
    cat_inner.append(&pref_action_row("打开索引编辑器", None, &cat_btn));
    {
        let w = win.clone();
        cat_btn.connect_clicked(move |_| {
            crate::ui::catalog_editor::open_editor(&w);
        });
    }
    content.append(&cat_card);

    // ------------------------------------------------------------------------
    // 数据
    // ------------------------------------------------------------------------
    let (data_card, data_inner) = pref_group("数据");

    let clear_btn = gtk4::Button::builder()
        .label("清除")
        .css_classes(vec!["destructive-action"])
        .build();
    data_inner.append(&pref_action_row(
        "清除本地会话记录",
        Some("删除所有对话历史，操作不可恢复。"),
        &clear_btn,
    ));
    content.append(&data_card);

    // ------------------------------------------------------------------------
    // 高级设置（v3.6.0 新增，折叠区）：服务端环境变量等专家选项默认收起。
    // 渐进披露：普通用户用不到的选项不应与高频设置争夺注意力；
    // 折叠标签明确「高级」语义，展开才可见，行为与系统设置的实现细节一致。
    // ------------------------------------------------------------------------
    let adv_toggle = gtk4::ToggleButton::builder()
        .label("高级设置")
        .halign(gtk4::Align::Start)
        .build();
    content.append(&adv_toggle);
    let adv_revealer = gtk4::Revealer::builder()
        .transition_type(gtk4::RevealerTransitionType::SlideDown)
        .reveal_child(false)
        .child(&srv_card)
        .build();
    {
        let rv = adv_revealer.clone();
        adv_toggle.connect_toggled(move |t| rv.set_reveal_child(t.is_active()));
    }
    content.append(&adv_revealer);

    // ------------------------------------------------------------------------
    // 帮助
    // ------------------------------------------------------------------------
    let (help_card, help_inner) = pref_group("帮助");

    let cl_btn = gtk4::Button::builder().label("查看").build();
    help_inner.append(&pref_action_row("更新日志", Some("查看各版本更新记录。"), &cl_btn));

    let wd_btn = gtk4::Button::builder().label("查看").build();
    help_inner.append(&pref_action_row("首次使用引导", Some("重新打开首次使用向导。"), &wd_btn));

    let dc_btn = gtk4::Button::builder().label("查看").build();
    help_inner.append(&pref_action_row("教学文档", Some("按场景整理的使用教学。"), &dc_btn));
    content.append(&help_card);

    // 内容超出窗口高度时可滚动（GNOME 设置页惯例，避免新增分区被裁切）
    let scroller = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&content)
        .build();
    let win_dialog = gtk4::Window::builder()
        .transient_for(win)
        .title("设置")
        .default_width(520)
        .default_height(720)
        .build();
    win_dialog.set_child(Some(&scroller));
    win_dialog.present();

    // 保存
    let win2 = win.clone();
    win_dialog.connect_close_request(move |_| {
        let mut s = Settings::load();
        // 地址规范化后保存（自动补协议、去尾斜杠），并把规范化结果回填输入框让用户可见
        let normalized = crate::config::normalize_base_url(&base_entry.text());
        base_entry.set_text(&normalized);
        s.base_url = normalized.clone();
        s.theme = match combo.selected() {
            1 => "dark".into(),
            2 => "light".into(),
            _ => "system".into(),
        };
        s.close_to_tray = tray_sw.is_active();
        // v1.0.21：启动与退出三项
        s.autostart = auto_sw.is_active();
        s.start_engine_on_launch = eng_sw.is_active();
        s.stop_engine_on_exit = exit_sw.is_active();
        if let Err(e) = set_autostart(s.autostart) {
            crate::ui::dialogs::show_err(&win2, &format!("开机自启动设置未生效：{e}"));
        }
        s.refresh_interval = refresh_spin.value_as_int().max(1) as u32;
        s.ollama_models_dir = models_entry.text().to_string();
        s.default_keep_alive = ka_entry.text().to_string();
        s.num_parallel = np_spin.value_as_int().max(0) as u32;
        s.max_loaded_models = ml_spin.value_as_int().max(0) as u32;
        s.gpu_visible = gpu_entry.text().to_string();
        // v1.0.7：写盘失败显式反馈，不再静默
        if !s.save() {
            crate::ui::dialogs::show_err(&win2, "设置保存失败：无法写入配置文件（磁盘已满或权限不足）。本次修改未被保存。");
        }
        // 立即应用主题模式，无需重启
        crate::ui::settings::apply_theme();
        // v1.0.7：地址有变更时明确告知已生效（生效环反馈缺口）
        if normalized != prev_base {
            crate::ui::dialogs::info(
                &win2,
                "服务地址已更新",
                &format!("新地址：{normalized}\n即时生效，无需重启应用。"),
            );
        }
        glib::Propagation::Proceed
    });

    // 清除会话：提示（win3 须在 close_request 捕获 win2 之前克隆）
    let win3 = win.clone();
    clear_btn.connect_clicked(move |_| {
        crate::ui::dialogs::info(&win3, "已清除", "会话记录已清空（重新选择模型即新会话）");
    });

    // 更新日志
    cl_btn.connect_clicked(move |_| {
        open_changelog(&win_dialog);
    });

    // 首次使用引导
    {
        let w = win.clone();
        wd_btn.connect_clicked(move |_| {
            open_welcome(&w);
        });
    }

    // 教学文档
    {
        let w = win.clone();
        dc_btn.connect_clicked(move |_| {
            open_docs(&w);
        });
    }
}

/// 教学文档（v2.0.0）：分节使用教学，覆盖连接 / 装模型 / 跑模型 / 参数 / 排障。
/// 与「首次使用引导」分工：引导管「第一次怎么上手」，本文档管「随时回来查」。
pub fn open_docs(win: &gtk4::Window) {
    let sections: &[(&str, &[&str])] = &[
        (
            "连接 Ollama 服务",
            &[
                "应用通过 HTTP 连接 Ollama（默认 http://localhost:11434）。「概览」页状态灯显示版本号即为已连接，显示「未连接」说明服务没起来。",
                "启动服务：概览页启动按钮，或菜单栏「操作 → 打开 Ollama 引擎」；连接远程机器的 Ollama 时，到「帮助 → 设置」改服务地址。",
                "想让开机就位：设置 →「启动与退出」，打开「开机自启动」与「应用启动时自动运行 Ollama 引擎」。",
            ],
        ),
        (
            "安装与管理 Ollama 引擎",
            &[
                "本机没装 Ollama：设置 →「Ollama 引擎」→「安装引擎」。应用会先下载官方安装脚本（ollama.com/install.sh）落盘，再请求管理员授权（pkexec）执行，脚本内容可事后审计。",
                "升级：引擎已装时按钮自动变为「检查更新」；官方脚本幂等，直接执行即就地升级到最新版，模型与配置不受影响。",
                "卸载：只删除引擎程序与系统服务，已下载的模型全部保留在磁盘；重装后可继续使用。",
                "连接远程机器的 Ollama 时，这里的引擎操作仍只作用于本机；远程机器请自行管理其引擎。",
            ],
        ),
        (
            "安装模型",
            &[
                "「云库」页内置 120 项精选模型（每条附一句话简介与硬件适配标注），也可搜索官方库全部模型；卡片按你的显存标「流畅 / 勉强 / 放不下」——不确定就挑「流畅」的。",
                "点「拉取」开始下载，下载完成后模型出现在「管理」页；大模型下载耗时较长，拉取期间可最小化到托盘干别的。",
                "「管理」页可复制模型名、查看详情（参数量/量化档位/能力）、删除不再需要的模型以释放磁盘。",
            ],
        ),
        (
            "使用模型",
            &[
                "「测试」页三合一：对话 / 生成 / 嵌入，页顶切换。对话页顶部下拉直接选已装模型，选完即用。",
                "模型首次被使用时会加载进显存/内存（第一次回复会慢几秒，之后变快）；一段时间不用会自动卸载，设置里可调 keep_alive 或「常驻」。",
                "「操作 → 关闭所有正在运行的模型」可一次释放全部显存。",
            ],
        ),
        (
            "生成参数怎么调",
            &[
                "temperature：随机性。0 附近稳定适合事实问答，0.7–1.0 适合写作创意。",
                "top_p / top_k：候选词过滤，一般不动；num_ctx：上下文长度，长文档/长对话调大（吃显存）；num_predict：最大生成长度。",
                "不确定就用默认。对话页「参数」对话框内置 创意 / 精确 / 快速 三档一键填充。",
            ],
        ),
        (
            "创建自己的模型",
            &[
                "「管理」页 → 从 Modelfile 创建：走 5 步向导（基本信息 → 参数 → 提示词 → 高级 → 确认），实时预览生成的 Modelfile。",
                "最简只填两处：模型名 + 基座模型（选已装的）；SYSTEM 提示词让模型固定人设/风格，例如「你是一个简洁的中文助手」。",
                "本地 GGUF 权重也能导入：第 1 步点「选择本地权重文件」。",
            ],
        ),
        (
            "出问题了怎么办",
            &[
                "界面未连接：确认服务在跑（概览页启动按钮）；地址改过就连不上——回设置测试连接。",
                "对话报 does not support chat：选到嵌入模型了，回「管理」页挑个不带「· 嵌入」标注的模型。",
                "响应慢 / 显存爆：换小参数量模型（云库看「流畅」徽章），或关掉其他已加载模型腾显存。",
                "应用卡死：等 30 秒看门狗会自动退出应用；着急可终端执行 kill -9 $(pidof offideskollama)。引擎不受影响，重开应用即可。",
                "服务日志在哪：菜单栏「帮助 → 服务日志」，排查 Ollama 本体报错靠它。",
            ],
        ),
    ];
    let dlg = gtk4::Window::builder()
        .transient_for(win)
        .modal(true)
        .title("教学文档")
        .default_width(720)
        .default_height(640)
        .build();
    let scroll = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();
    let vbox = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(6)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(20)
        .margin_end(20)
        .build();
    let intro = gtk4::Label::builder()
        .label("按场景整理的使用教学；「首次使用引导」管第一次上手，本文档随时回来查。")
        .halign(gtk4::Align::Start)
        .wrap(true)
        .css_classes(vec!["dim-label"])
        .build();
    intro.set_xalign(0.0);
    vbox.append(&intro);
    for (title, items) in sections {
        let head = gtk4::Label::builder()
            .label(*title)
            .halign(gtk4::Align::Start)
            .css_classes(vec!["title-4", "heading"])
            .margin_top(10)
            .build();
        vbox.append(&head);
        for it in *items {
            let body = gtk4::Label::builder()
                .label(format!("• {it}"))
                .halign(gtk4::Align::Start)
                .valign(gtk4::Align::Start)
                .wrap(true)
                .wrap_mode(gtk4::pango::WrapMode::WordChar)
                .selectable(true)
                .build();
            body.set_xalign(0.0);
            body.set_margin_bottom(4);
            vbox.append(&body);
        }
    }
    scroll.set_child(Some(&vbox));
    dlg.set_child(Some(&scroll));
    dlg.present();
}

/// 把「测试连接」的失败翻译成用户可行动的原因（v1.0.7：不再丢弃 reqwest 错误细节）。
pub fn connect_failure_reason(res: &Result<bool, reqwest::Error>) -> &'static str {
    match res {
        Ok(true) => "",
        Ok(false) => "已连上，但服务返回了错误状态（该地址可能不是 Ollama 服务，或被反向代理拦截）",
        Err(e) if e.is_timeout() => "连接超时（对端不可达，或防火墙静默丢包）",
        Err(e) if e.is_connect() => "无法建立连接（地址或端口写错、对端 Ollama 未运行、或 https/http 协议不匹配）",
        Err(e) if e.is_decode() => "响应不是有效数据（该地址上运行的可能不是 Ollama）",
        Err(_) => "请求失败（网络不可用或请求被中断）",
    }
}

/// 更新日志板（版本更新记录）
pub fn open_changelog(win: &gtk4::Window) {
    let logs = changelog_entries();
    changelog_window(win, &logs);
}

fn changelog_window(win: &gtk4::Window, logs: &[(&'static str, &'static str)]) {
    let dlg = gtk4::Window::builder()
        .transient_for(win)
        .modal(true)
        .title("更新日志")
        .default_width(560)
        .default_height(620)
        .build();
    let scroll = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();
    let vbox = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(6)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(20)
        .margin_end(20)
        .build();
    for (ver, desc) in logs {
        let head = gtk4::Label::builder()
            .label(*ver)
            .halign(gtk4::Align::Start)
            .css_classes(vec!["title-4", "heading"])
            .margin_top(10)
            .build();
        vbox.append(&head);
        let body = gtk4::Label::builder()
            .label(*desc)
            .halign(gtk4::Align::Start)
            .valign(gtk4::Align::Start)
            .wrap(true)
            .wrap_mode(gtk4::pango::WrapMode::WordChar)
            .selectable(true)
            .build();
        body.set_xalign(0.0);
        body.set_margin_bottom(4);
        vbox.append(&body);
    }
    scroll.set_child(Some(&vbox));
    dlg.set_child(Some(&scroll));
    dlg.present();
}

/// 更新日志原始条目（v1.0.19：设置窗入口与「查看→信息」弹窗共用同一数据源）
fn changelog_entries() -> Vec<(&'static str, &'static str)> {
    vec![
        ("v3.6.2", "修复「下载模型经常在不同阶段失败，报 [Api] EOF」（用户实测反复踩中）：根因是 Ollama 的 blob 下载会分段写盘，一旦中途被中止（关窗 / 断网 / 重启服务 / 进程被杀），模型目录 blobs/ 下会残留 `-partial` 与 `-partial-N` 未完成分片；下次拉取同一模型时 Ollama 撞上这些不一致状态便直接断开，流内只回 error=\"EOF\"（HTTP 层仍是 200），旧实现原文透传，用户只看到 `[Api] EOF` 三个字母，且每次失败阶段都不同、看似随机，完全无从下手。① 自动自愈——拉取遇裸 EOF 时自动清理残留分片并重试一次，用户无感；② 错误翻译——识别裸 EOF / unexpected EOF 等变体，改为说明成因（中断残留）与下一步动作，不再原文透传；③ 一键修复入口——下载失败弹窗附「清理残留并重试」按钮，清理失败（残片属主为 ollama 服务账户、普通用户删不掉）时自动走图形授权提权清理；④ 明确告知已装好的模型完全不受影响，消除「是不是又把我模型搞坏了」的顾虑"),
        ("v3.6.1", "修复「引擎以 systemd 专用账户运行后，界面看不到本地模型」（用户实测踩中）：Ollama 官方安装脚本会新建 `ollama` 系统账户、systemd 单元带 User=ollama，服务于是读取 /var/lib/ollama，而用户模型在 ~/.ollama/models —— 服务与 API 全部正常，但 GET /api/tags 返回空数组，用户误以为「模型被删了」。① 根因修复——引擎安装/升级完成后自动对齐模型目录（写 systemd drop-in 注入 OLLAMA_MODELS，并放开家目录穿透权限），既有模型零搬动直接接回；② 主动诊断——服务在跑但模型为 0 时自动比对用户模型目录，磁盘上确有模型则弹窗说明原因、澄清「模型没有被删除」并给出修复入口；③ 托盘「运行 Ollama 引擎」勾选态只认服务可达性（原 up && ver.is_some() 在版本请求慢/超时时勾不上，表现为「运行引擎失效」）；④ 启动服务改为轮询确认真实就绪（原 systemctl start 退出码 0 即算成功，Restart=always 下「启动即崩溃」也会报成功且无任何提示），失败时透出逐级真实原因"),
        ("v3.6.0", "开源前整备版本：① 内置模型目录 68→120 项（中文系+国际系精选，每条附一句话简介；修正 qwen2.5-vl→qwen2.5vl 官方库名（旧名会拉取失败）；搜索支持中文别名与描述双路匹配）；② 设置页新增「Ollama 引擎」管理：安装/检查更新/卸载（官方脚本先落盘再 pkexec 执行、随机文件名+排他创建+0700 权限防替换、卸载保留全部模型数据）；③ 设置页信息架构重排：高频组置顶，环境变量等专家选项收进「高级设置」折叠区；④ 首次使用引导与教学文档同步更新（引擎安装/升级/卸载说明、120 项目录）；⑤ 稳定性加固：模型体积排序改 total_cmp、自启动路径防御性校验、互斥锁中毒自动恢复等 5 处潜在崩溃点消除"),
        ("v3.5.3", "修复对话/生成时模型不支持「思考」模式却裸 JSON 报错的问题：当开启输入区上方「思考」开关，而所选模型（如 codellama:7b）并非推理模型时，Ollama 返回 `\"xxx\" does not support thinking`，旧实现只翻译了 `does not support chat` / `does not support generate`（嵌入模型误入对话/生成），没覆盖 thinking，导致用户看到 `生成失败：[Api] 聊天请求失败：{\"error\":\"...does not support thinking\"}` 这类裸 JSON。现扩展 `friendly_capability_error` 把 thinking 能力错误也翻译成中文可行动提示：指出当前模型不是推理模型、请关闭「思考」开关后再试。纯 bug 修复，无功能变更"),
        ("v3.5.2", "修复「开机自启动」实际失效（v1.0.21 引入、一直未被发现，因默认关闭）：根因——GtkApplication 不认识 --tray 参数，offideskollama --tray 直接报「未知选项 --tray」退出，而写入 ~/.config/autostart 的自启动条目 Exec 正是带这个参数，于是登录后静默启动到托盘这一步从未成功过。修法——在 gtk4::Application 上注册 --tray 为已知选项并接管 command-line 信号：激活应用、消费掉全部参数（含 --tray），不再让默认选项解析把未知参数当错误拒绝；窗口是否弹出仍由 start_hidden()（读取 env args 中的 --tray）决定，行为保持不变。纯 bug 修复，无功能变更"),
        ("v3.5.1", "修复推送模型时的「操作失败 [Api] 403: {\"errors\":[{\"code\":\"ANONYMOUS_ACCESS_DENIED\"...}]}」原始 JSON 报错：① 根因——推送/拉取失败时 Ollama 仍返回 HTTP 200，真实原因放在 NDJSON 流的 error 字段里（形如 `403: {...}`），而 v3.2.3 的错误翻译只挂在「HTTP 非 2xx」分支上，该分支对推送**永远不会触发**，所以修了等于没修；现新增流内错误翻译 `api_from_stream_error`，拉取/推送/创建三处流内 error 统一走它，命中注册中心错误码即给出可行动中文提示（提示执行 `ollama login`）；② 兜底加固——HTTP 401/403 但体里识别不出具体错误码时，同样给可行动提示，不再把原始 JSON 抛给用户；③ 名称不规范（NAME_INVALID）的提示补上解决办法：推送需带账户命名空间，如 `用户名/模型名:标签`"),
        ("v3.5.0", "云库系统重构（按「推荐层 + 检索层」双层模型，逐条修复源码走查发现的缺陷）：① 已安装状态改为按完整标签精确判定——旧实现按模型基名匹配，装过 qwen2.5:0.5b 会把推荐 7b 的卡片也标成「已安装」，属状态误报，现精确到标签并在卡片列出本机其它标签；② 下载完成后卡片自动回填——拉取流程新增完成回调，成功/失败/取消都会通知云库页刷新（旧实现下载完仍显示「安装」，需手动点刷新或离开页面再回来）；③ 拉取/推送进度框改非模态 + 新增「取消」按钮，取消即断开下载流并中止服务端下载（旧实现是全屏模态且无任何按钮，下载大模型期间整个应用被挡住且无法中断）；关闭进度窗口等同于取消；④ 安装前新增确认框，显示完整模型名、预计体积与目标分区剩余空间，空间不足直接拒绝并给出所需/可用对比（旧实现一键直下、无确认也不检查磁盘）；⑤ 官方库检索并入云库页——搜索框查本地精选，零命中时给出明确空状态与「在 Ollama 官方库搜索」出口，官方结果页内呈现标签/体积/适配/安装（旧实现真检索藏在管理页侧栏，云库搜不到就是一片空白，用户会误判模型不存在）；⑥ 新增排序（默认/适配优先/体积升降/名称）；⑦ 对比勾选状态跨重建保留，超过 3 个时回退勾选并明确提示（旧实现一搜索就丢失勾选、超上限静默失败）；⑧ 适配徽章去掉硬编码色值改用 symbolic 图标 + 主题 .warning/.error——旧写死的橙色配白字对比度约 4.2:1，低于 WCAG 2.1 AA 正文 4.5:1；⑨ 「已安装」分类改为直接展示本机真实条目（含体积/量化），自建 Modelfile 模型不再漏掉；⑩ 详情页参考估算行补安装按钮，离线不再是死路；⑪ 搜索防抖 150ms、页脚去掉与硬件行重复的信息"),
        ("v3.4.0", "设置页原生化重设计：按 Cinnamon / GNOME 系统设置惯例，把零散设置项分组为 .frame 卡片；行内控件按「标题在左、操作在右」排布，开关/动作行使用近似 rich-list 的横向行布局；连接地址的输入框与测试按钮套用 .linked 成组；所有排版继续沿用本机主题确实定义的 .title-4/.heading/.caption/.dim-label，不引入 .card/.boxed-list/.preferences 等无效类。功能与设置项全部保留，仅调整视觉层级与分组"),
        ("v3.3.0", "云库索引可编辑 + 索引扩充：① 云库索引从编译期常量改为「内置种子 + 用户叠加」双层结构——内置清单随版本升级自动获得新增模型（本次由 36 条扩充至 68 条），你在「设置 → 云库索引」里的增改只写入 ~/.config/offideskollama/catalog_user.json 叠加层，不改内置本体；② 新增图形化索引编辑器，按 GNOME 可编辑列表规范：行尾编辑/删除、内置条目可单项恢复、底部「添加模型」行，并支持恢复默认、导入/导出 JSON；③ 云库页原生化微改造——主导航套用 .navigation-sidebar（Cinnamon/Mint-Y 原生侧栏选中态）、卡片改用主题确有的 .frame 边框、工具条与成组控件改用 .toolbar/.linked，并清理主题中不存在因而无效的 .card/.monospace/.success 等样式类"),
        ("v3.2.2", "修复大模型拉取到末尾报「[Parse] error decoding response body」的问题：根因是 /api/pull 流末尾被 Ollama 对端切断（连接重置/异常关闭）时，reqwest 的流传输错误被我们错误归类为 Parse；现改为① 流错误归类为 Network，错误提示更准确；② pull/push/create 使用无整体超时的独立客户端，避免慢网络下大模型被 10 分钟总超时切断；③ 拉取流异常结束后二次校验 /api/tags，若模型已实际落盘则按成功处理，不再弹报错框"),
        ("v3.2.1", "对话页布局修复：会话侧栏与对话区改用 Paned 分栏——侧栏初始固定 224px 且禁止被挤压，中间分界线可手动拖动调节比例，拖定后保持绝对位置，窗口缩放时侧栏不再跑动"),
        ("v3.2.0", "系统化 UI 与交互升级：① 菜单栏改走 ApplicationWindow 内建菜单栏（GTK4 唯一原生路径），由 Cinnamon 主题直接渲染，与 Nemo 等系统应用视觉一致；② 托盘菜单分组重排——新增只读状态行（Ollama 版本/模型数/已加载数）、引擎改为勾选型开关（勾选态=运行中）、新增「打开云库」「教学文档」快捷入口，并支持悬浮 tooltip 显示引擎状态；③ 左侧导航加 symbolic 图标；④ 对话页新增流式生成状态条（旋转指示 + 生成中…）；⑤ 快捷键体系：Ctrl+1~4 切换主页面、Ctrl+F 转云库、F5 刷新模型列表（原有 Ctrl+, 打开设置）；⑥ 云库「为你选」按钮降权为普通样式，收敛首行视觉层级"),
        ("v3.1.1", "修复点「附加图片」（多模态）等文件选择对话框时应用闪退的问题：File Chooser 对话框在 show() 后引用被立即释放，GTK 在对话框可见期间将其销毁引发段错误（dmesg 可见 libgtk-4 segfault，托盘图标随进程退出消失）。现把对话框引用移入 response 回调保活，同修 4 处：测试→对话「附加图片」、对话导出、生成页「附加图片」、创建模型向导选权重文件。纯修复，无功能变更"),
        ("v3.1.0", "对话页重构：会话不再藏在顶部下拉框里——新增左侧会话侧栏（ListBox 原生列表），每条会话双行显示标题与所用模型，点击切换、行内删除，头部「新建」按钮一键开新对话；「参数」从底部按钮行提权到顶栏与模型选择同级；底部操作行精简为 清空/导出/重新生成 与 停止/发送；移除对话页顶部旧的六按钮操作行（新建/重命名/删除/清空/导出/重新生成），功能入口统一收进侧栏与对话面板。纯界面重构，会话持久化与全部对话能力不变"),
        ("v3.0.2", "修复「主题」三档切换在 Mint-Y 主题家族下全部无效的问题：原实现仅依赖 gtk-application-prefer-dark-theme 开关，该开关要求主题存在 <名称>-dark 变体，而 Mint-Y 家族（Mint-Y-Aqua 等）的深色版是独立主题目录（Mint-Y-Dark-Aqua），变体缺失导致选深色/浅色均无反应。现改为深/浅档直接将本应用主题切换到对应变体目录（仅本应用生效，不改系统主题），「跟随系统」还原本机主题并继续实时跟随桌面设置；Adwaita 等自带 -dark 变体的主题兼容不变"),
        ("v3.0.1", "修复两处 3.0.0 遗留问题：① 「测试 → 对话」页内容区空白——对话模块部件被同时挂载到主窗口 Stack 与测试页 Stack 两处，GTK4 拒绝二次挂载致页面全空，现仅由测试页挂载；② 任务栏/启动器应用图标丢失——桌面文件 StartupWMClass 残留旧名大小写（offideskOllama），与实际进程名 offideskollama 不匹配导致任务栏无法将窗口与图标配对，已统一为全小写。纯修复版本，无功能变更"),
        ("v3.0.0", "更名「O-deskUI .wokk.」（中文名：Ollama管理器WokK）并升 3.0.0。窗口标题、托盘菜单、退出确认、应用信息、自启动条目、桌面文件 Name/GenericName 同步更名；二进制与包名保持 offideskollama 以兼容既有安装路径与配置"),
        ("v2.0.0", "更名「O Desk WOKK」（中文名：Ollama 管理器 WOKK）并升 2.0.0。① 新增「教学文档」：按场景整理的完整使用教学（连接 / 装模型 / 使用模型 / 生成参数 / 创建模型 / 排障），入口在设置窗，首次使用引导完成页亦有直达按钮；② 顶栏菜单栏样式修正——GTK4 已无传统菜单栏控件，此前 PopoverMenuBar 在 Mint-Y 主题下继承按钮底色边框，看起来像一排按钮，现以应用级 CSS 修成与系统应用（如 Nemo 文件管理器）一致的扁平文本菜单栏，悬停高亮；③ 窗口标题、托盘菜单、退出确认、应用信息、自启动条目同步更名。二进制与包名保持 offideskollama 以兼容既有安装路径与配置"),
        ("v1.0.22", "向导系统重做（两套）：① 新增通用分步向导框架（步骤条 + 上一步/下一步 + 逐步就地校验 + 底部「第 x/y 步」），首次使用引导与创建模型向导共用；② 首次使用引导——旧版还在教 1.0.16 之前已删除的界面（左侧模型列表 / 聊天·生成·已加载·嵌入 / 顶栏启停），属教错用户，现改为当前界面的真实导览（概览·管理·云库·测试 + 顶部菜单栏），并可从「帮助 → 首次使用引导」或设置窗底部随时重看；③ 创建模型向导——旧版名为向导实为四个 Notebook 标签页（任意跳页、无导航、校验全堆在最后的「创建」按钮上、预览要手动点刷新），现改为 5 步线性引导，每步离开前就地红字提示（不弹窗、不跳页），Modelfile 预览随填写实时更新"),
        ("v1.0.21", "新增「启动与退出」三项开关（设置内）：① 开机自启动——登录后静默启动到托盘不弹窗口（写入/移除 ~/.config/autostart 条目，带 --tray 参数）；② 应用启动时自动运行 Ollama 引擎——仅在引擎未运行时后台拉起，已运行则不打扰，减少每次手动启动的繁杂操作；③ 退出应用时同时关闭 Ollama 引擎——托盘退出或关窗退出时联动停止引擎（关闭则引擎照常后台常驻）。三项均默认关闭，行为由用户决定"),
        ("v1.0.20", "热修（长期未决的「启动数分钟后必卡死」根因确证并修复）：已加载面板的自动刷新定时器存在指数倍增缺陷——每次触发既新建一个定时器、自身又周期续期，数量每 8 秒翻一倍（启动 3 分钟后超 3 万个），每个都执行 HTTP 轮询 + 同步读配置文件，主循环被灌满 → 100% CPU、界面冻结、托盘无响应。因与具体操作无关、随时间必然发生，故表现为「任何按钮都有概率触发」。本版删除该废弃面板（职能自 1.0.16 起已由概览页承担），连根去除倍增源。带符号黑匣子立功：三次冻结的主线程栈全部定格在同一处文件读取，一击定罪"),
        ("v1.0.19", "顶部原生菜单栏（查看/转到/操作/帮助）：查看含主题（跟随系统/深/浅）与信息弹窗（本机硬件 + 应用版本 + 本版本更新日志 + 跟随硬件的建议模型参数）；转到现在：主页→概览、模型管理→管理页；操作含 Ollama 引擎开关（标签随运行状态切换「打开/关闭」）与关闭所有运行中的模型；帮助收纳设置/服务日志/注册中心登录登出，替代原 ☰ 按钮"),
        ("v1.0.18", "侧栏名称精简：模型管理→管理、模型云库→云库、运行测试→测试"),
        ("v1.0.17", "对话页新增模型下拉：运行测试 → 对话页顶部可直接下拉选择本地已装模型（自动加载列表 + 刷新按钮），不再必须绕道「模型管理」页选模型；与侧栏选择双向同步，换模型保留会话历史的行为不变"),
        ("v1.0.16", "信息架构重设计（用户主导）：① 导航 8 项收敛为 4 项——概览(含服务控制)/模型管理/模型云库/运行测试；② 模型管理与模型云库拆为两个独立页面，不再 Paned 并排互相干扰（模型管理为本应用主功能，云库为其重点延伸）；③ 对话/生成/嵌入合并为「运行测试」页内切换；④ 服务页并入概览，删除「最近会话」只读摘要；⑤ 设置只保留菜单一个入口（撤销侧栏/顶栏重复入口），更新日志撤专属入口、收敛到设置窗内"),
        ("v1.0.14", "卡死黑匣子升级（现场抓捕+自动止损）：实机 watchdog.log 证实卡死时主线程处于用户态 CPU 空转（syscall=running），内核层信息只能看到这一层——本版卡死时向全部线程发信号、各线程自抓 Rust 调用栈写入日志，下次卡死即可精确定位到函数；持续卡死超 30 秒自动退出应用并释放托盘，不再需要重启系统（卡死 30 秒内自行恢复则不打扰）"),
        ("v1.0.12", "诊断+修复（卡死黑匣子与更新日志窗）：① 新增卡死黑匣子——主循环心跳监测，卡死超 6 秒时看门狗自动把所有线程的阻塞点写入 ~/.cache/offideskollama/watchdog.log，下次卡死把该文件发回即可精确定位，不再「只能重启」却无证据；② 更新日志窗改用可滚动窗口（原先 info 弹窗无滚动条，长文本超出屏幕、按钮点不到）"),
        ("v1.0.10", "修复（嵌入模型误入对话页，实测 bge-m3 报 does not support chat）：① 侧栏模型列表探测能力（/api/show capabilities）——仅嵌入模型名字后标注「· 嵌入」，一眼可辨；② 对话发送前拦截——选中仅嵌入模型时直接提示「不支持对话，请选对话模型/嵌入模型去嵌入页」，不再发起注定失败的请求；③ 报错翻译兜底——若旧版 Ollama 无能力字段漏拦，「does not support chat/generate」的原始报错翻译为可行动提示（含模型名与指路），聊天与生成两处同修。诚实边界：探测失败不缓存不拦截（宁可漏拦不可误拦，普通 LLM 含 completion 永不误标）"),
        ("v1.0.9", "热修（模型库页拉取时卡死）：根因是进度事件风暴——Ollama 的 /api/pull 对每个下载块都发一行进度（快连接下每秒数百上千条），1.0.4 事件驱动桥接后每条都直接唤醒主线程更新进度框，主循环被灌满 → 界面卡死。修复：① 在源头节流（ProgressThrottle）——首次、阶段切换、≥1% 跳变、≥250ms 间隔或完成时才上报，事件量降 200 倍，进度观感无损（拉取/推送同修）；② HTTP 客户端改为全局共享连接池——此前每次请求都新建客户端（含 TLS 初始化），3 秒轮询每次建 3-4 个，长时间运行句柄无谓增长；③ 模型详情的 registry 逐标签取体积加短路——registry 不可达（大陆常见）时首个失败即其余标「未知」，不再 30+ 标签 × 10 秒串行等待造成详情窗假死，单标签超时 10s→3s。新增节流器单测 4 项"),
        ("v1.0.8", "收尾（连接流程审计剩余项全修）：① 服务页远程/本地语义分区——地址指向远程时，状态灯如实标注「（远程 地址）」，启停按钮转为停用并说明只管理本机，附一条分区说明（本地地址自动隐藏）；② 地址输入兜底升级为自动修正并回填可见——全角冒号（：→:）、纯端口数字（11434→localhost:11434）、子路径剥离（http://h/ollama→http://h，Ollama 不支持 base path）、只写协议（http://→补 localhost）；③ 诚实边界：https 误写仍不擅自降级（靠测试连接协议提示），无括号 IPv6 与带认证 URL 行为不变（已有单测锁定）。地址规范化边界测试更新为 9 项断言（由记录缺口改为锁定自动修正行为）"),
        ("v1.0.7", "热修（连接流程四项未闭环，全链路审计产出）：① 测试连接失败不再只给通用清单——分类显示真实原因（超时/无法建立连接/HTTP 错误状态/响应异常），地址带 https:// 时追加「对端未启用 HTTPS 则改 http://」提示；② 服务页启停加远程守卫——地址指向远程机器时点启停会明确说明只能操作本机服务，不再做无意义操作；③ 设置保存失败（磁盘满/权限）显式报错，不再静默丢弃；④ 服务地址有变更时保存后明确提示「已生效，无需重启」，无变更不打扰。新增 7 项单元测试"),
        ("v1.0.6", "界面重构（模型库页）：默认窗口升级为 1280×800（原 1080×680 放不下三层结构）；发现中心改为三行分区——搜索+为你选+对比 / 分类下拉+任务筛选 / 硬件状态+低频操作（刷新已装·重测硬件·填显存下沉为文字按钮）；原左侧 10 项分类列表收敛为下拉框，省出一整列；卡片区固定最多 2 列不再挤压"),
        ("v1.0.5", "热修（改地址连不上）：服务地址保存时自动规范化（漏写 http:// 自动补、去尾斜杠），写 192.168.1.5:11434 这类简写也能正常使用；设置页新增「测试连接」按钮，改完地址先试再关窗，连不上时给出具体排查步骤（地址端口 / 对端 OLLAMA_HOST=0.0.0.0 / 防火墙）"),
        ("v1.0.4", "热修（卡死/未响应）：重写异步桥接层——后台任务结果改为事件驱动送回界面线程，废除每秒上百次的轮询定时器；状态轮询全部加防重入保护（上一轮未完成则跳过）。此前 Ollama 忙或网络慢时轮询任务堆积、轮询定时器淹没界面主循环，表现为「未响应」、托盘失效、只能重启；本版从根源消除。功能与 1.0.3 一致"),
        ("v1.0.2", "热修（安装卡顿）：安装后脚本去掉 gtk-update-icon-cache 的 -f 强制全量重建标志——hicolor 全量重建在部分机器上耗时分钟级，表现为 apt 安装进度停在中途不动；改为增量更新（秒级）且全程 || true 兜底，即使缓存更新失败也不影响安装。应用功能与 1.0.1 完全一致"),
        ("v1.0.1", "热修：① 操作流归位——全局 ☰菜单不再聚合会话操作（新建/重命名/删除/清空/导出/重新生成），全部移入「对话」模块页顶部操作行，与会话下拉框同屏同位；全局菜单只留系统级操作（服务启停/服务日志/注册中心/设置/更新日志）；② 修闪退隐患——退出守门改为全程 try_borrow，流式回复回调期间关窗不再可能因借用冲突 panic；概览页空态提示改为每轮新建控件，杜绝控件重复挂载导致的 GTK panic；会话摘要 pango 标记改用通用 foreground 写法"),
        ("v1.0.0", "0.17.0 管理控制台重设计落地：① 信息架构重排——底部 Tab 切换改为左侧「概览/模型库/对话/生成/嵌入/运行时/服务/设置」八模块导航，模型列表与发现中心并排归入「模型库」，聊天降为平级模块之一（本程序定位：Ollama 全能桌面管理器）；② 新增「概览」驾驶舱（默认页）——服务状态（含 systemd/进程托管方式）/ Ollama 版本 / 已加载数 / 显存占用四张状态卡 + 已加载·运行时面板（keep_alive 倒计时、行内「常驻/卸载」）+ 最近会话摘要，全部来自统一轮询的真实状态；③ 「服务」模块页——服务启停按钮与状态灯自顶栏移入，附服务日志入口与失败诊断说明；④ 状态栏增列服务托管方式、连接状态与模型总数；⑤ 退出守门——有生成中回复或拉取中模型时，退出前列出将中断的任务并推荐「最小化到托盘」；⑥ 顶栏精简（服务按钮移入服务页，功能一项不减）"),
        ("v0.16.0", "新增「模型发现中心」（主窗口第 5 个「发现」页，产品亮点模块）：① 分类浏览——对话/代码/推理/视觉/嵌入/中文优化/小模型/已安装/编辑推荐，支持搜索与中文别名（千问→qwen、羊驼→llama、智谱→glm…）；② 硬件感知——自动检测显卡/显存/内存/CPU 核数，每个模型卡片标「流畅/勉强/放不下/内存可跑」，检测不到显存时可手动填，绝不编造；③ 任务导向推荐——顶部智能 chips（聊天/写代码/读图/嵌入/强推理/老机器）按能力×硬件 fit 排序，并「为你选」一键安装主对话+推理+嵌入起步套餐；④ 量化扫盲——详情页解释 fp16/q8_0/q4_K_M 等档位，并用条形图展示相对体积与质量启发式估算（已标注非实测）；⑤ 对比视图——勾选最多 3 个模型逐行对比体积/适配/能力并高亮胜出；⑥ 已装同步——查询本地已装模型并在卡片标「已安装」徽章。所有体积均为公开已知参考值，详情页联网时用 registry 真实体积覆盖，全程不显示任何伪造下载量"),
        ("v0.15.1", "热修：模型库浏览器改从 ollama.com/library/{模型}/tags 公开页面解析标签（registry 的 /tags/list 端点官方未暴露且大陆访问受限），大小从 registry manifest 尽力获取，不可达时显示「未知」；增加请求 User-Agent；输入模型名时若误带 :标签 会自动取基座名"),
        ("v0.15.0", "能力补齐（对照原生 Ollama 操作）：① 托盘修复——启动托盘改用 assume_sni_available，桌面 SNI 宿主未就绪时也能正常最小化到托盘（此前 SNI 未就绪会导致关闭窗口直接退出而非最小化）；② 自定义 JSON Schema 输出格式——对话格式下拉新增「自定义 Schema」，可粘贴 JSON Schema 让模型按结构输出；③ 文本嵌入批量——嵌入面板支持一次多条（每行一条），逐条显示维度与样本；④ 服务端配置——设置新增「服务端配置」分组（模型目录 OLLAMA_MODELS / 默认 keep_alive / 并发请求数 OLLAMA_NUM_PARALLEL / 并行加载模型数 OLLAMA_MAX_LOADED_MODELS / 可见 GPU），改后重启服务生效；⑤ 模型库浏览——侧栏新增「浏览模型库」按钮，输入模型名查询官方库标签并点选拉取；⑥ Modelfile 引导向导——创建模型从纯文本框升级为多页引导（基座模型下拉选已装 / 手填 / 选本地权重文件 + 参数可视化 + SYSTEM/TEMPLATE 分栏 + ADAPTER/LICENSE 高级），实时预览生成的 Modelfile；⑦ 多模型并行——由服务端并发配置（④）与既有多会话支撑，可同时驻留多个模型对话"),
        ("v0.14.0", "增值层补齐：① 多会话持久化——下拉框切换 / 新建 / 重命名 / 删除，换模型保留每条会话历史，会话存于 ~/.config/offideskollama/sessions.json；② 参数预设——生成参数对话框内置「创意 / 精确 / 快速」一键填充常用采样档位；③ 服务日志窗——菜单「服务 → 查看日志」读取 journalctl -u ollama 与 ~/.ollama/logs/server.log，可刷新；④ 生成面板支持多模态图片输入（与聊天一致，附图随本条提示发送）；⑤ 会话菜单新增新建 / 重命名 / 删除入口"),
        ("v0.13.0", "架构与能力大升级：① 移除 libadwaita，纯 GTK4 控件，外观跟随 Cinnamon / Mint-Y 系统 GTK 主题（彻底解决「不跟随桌面、与系统割裂」问题）；② 思考过程展示（💭）与多模态图片输入（聊天可附图，随本条消息发送）；③ 生成参数扩充至覆盖 Ollama 原生指令集——keep_alive / think / format(json) / stop / min_p / typical_p / presence_penalty / frequency_penalty / penalize_newline / num_gpu 等；④ 已加载面板增加 keep_alive 倒计时与「常驻」钉选（一键常驻显存）；⑤ 修复停止模型接口字段错配（name→model）导致卸载静默失败；⑥ 新增菜单（会话 / 服务 / 视图 / 帮助，Ctrl+, 打开设置）与桌面通知（拉取完成 / 卸载 / 常驻 / 服务错误）；⑦ Markdown 强调色随系统深浅主题"),
        ("v0.12.0", "修复「启停按键无效」（三个叠加原因：ToggleButton 的 active 在 clicked 时已翻转导致启停逻辑颠倒；is_running 内部嵌套 tokio runtime 引发 panic 使后台线程静默死亡、状态永不刷新；systemctl 需要 root 且无降级方案）。现改为：纯同步状态检测、以真实状态驱动动作、systemctl→pkexec→直接启动 ollama serve 逐级降级、每 3 秒同步服务状态与版本、失败显式报错。能力增强：新增停止生成（按钮 / Esc）、生成统计（token 数、tokens/s、耗时，对齐 ollama run --verbose）、会话管理（清空 / 导出 Markdown / 重新生成）、流式失败显式提示；模型详情补齐许可证与模板，改为可滚动可复制窗口，并修复详情请求忽略自定义服务地址的问题；底部状态栏实时显示已加载模型数与显存占用"),
        ("v0.11.1", "修复启动崩溃：Markdown 文本标签（weight/style/underline/foreground-rgba）原用泛型属性写法导致类型不匹配、启动即中止；改为类型安全的 TextTag setter，启动恢复正常。功能与 0.11.0 一致"),
        ("v0.11.0", "全功能达标（对照 0.5.0 并增强）：配置独立模块且服务地址即时生效；拉取/推送显示真实进度百分比；聊天生成参数（system/temperature/top_p/top_k/num_ctx/num_predict/seed/repeat_penalty）完整可调；已加载模型自动刷新间隔可配置；SSD 系统标题栏适配 Cinnamon / GNOME / Ubuntu；主题切换即时生效"),
        ("v0.10.1", "应用更名为 offideskOllama；GTK4 原生版全功能"),
        ("v0.10.0", "GTK4 原生版：系统原生 UI、模型管理、聊天/生成/已加载/嵌入、系统托盘、设置、首次引导"),
        ("v0.5.0", "彻底修复按键失效；新增设置面板（服务地址/主题/刷新间隔/托盘行为/清会话）"),
        ("v0.4.0", "新增「设置」块：Ollama 地址、主题、刷新间隔、托盘行为、清除会话"),
        ("v0.3.0", "修复界面按键全部失效；修复卸载残留；系统托盘；模型操作收进竖三点+右键；界面全面汉化"),
        ("v0.2.0", "更换应用图标为 Ollama 官方 logo；新增「更新日志」面板"),
        ("v0.1.0", "首版：完整控制台、模型管理、流式聊天、系统主题跟随、首次使用引导"),
    ]
}

/// 更新日志正文（v1.0.19：供设置窗入口与「查看→信息」弹窗共用）——纯文本，跟随硬件建议一并展示
pub fn changelog_text() -> String {
    changelog_entries()
        .iter()
        .map(|(ver, desc)| format!("[{ver}] {desc}"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 设置主题模式并即时应用（v1.0.19 菜单栏「查看→主题」使用）
pub fn set_theme_mode(mode: &str) {
    let mut s = Settings::load();
    s.theme = mode.to_string();
    s.save();
    apply_theme();
}

/// 开机自启动条目路径（~/.config/autostart/offideskollama.desktop）
fn autostart_path() -> std::path::PathBuf {
    let dir = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::Path::new(&dir)
        .join(".config")
        .join("autostart")
        .join("offideskollama.desktop")
}

/// 应用/撤销开机自启动（v1.0.21）。
/// 条目带 `--tray` 参数：登录后应用静默启动到托盘，不弹主窗口。
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let path = autostart_path();
    if !enabled {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            // 本来就没有 = 已是目标状态
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("移除自启动条目失败：{e}")),
        };
    }
    let exe = std::env::current_exe().map_err(|e| format!("定位应用可执行文件失败：{e}"))?;
    let dir = path.parent().ok_or("自启动条目路径异常（缺少父目录）")?;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("创建 autostart 目录失败：{e}"))?;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=O-deskUI .wokk.\n\
         Comment=Ollama 图形化控制台（静默启动到托盘）\nExec={} --tray\n\
         Icon=offideskollama\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    std::fs::write(&path, entry).map_err(|e| format!("写入自启动条目失败：{e}"))
}

/// 应用信息 + 建议模型参数弹窗（v1.0.19 菜单栏「查看→信息」使用）
pub fn app_info_dialog(win: &gtk4::Window) {
    let prof = crate::ollama::hardware::load_hardware().unwrap_or_else(crate::ollama::hardware::detect_hardware);
    let vram = prof.vram_gb();
    // 建议参数跟随硬件：显存决定上下文长度档位与并发，内存兜底
    let (ctx, parallel, keep) = if vram >= 16.0 {
        ("16384", "4", "常驻（-1）")
    } else if vram >= 8.0 {
        ("8192", "2", "30 分钟（30m）")
    } else if vram >= 4.0 {
        ("4096", "1", "10 分钟（10m）")
    } else {
        ("2048", "1", "5 分钟（5m）")
    };
    let tip = if prof.vram_bytes.is_none() {
        "未检测到独立显存——将以内存/CPU 推理为主，建议选择小参数量模型（0.5B–3B）。"
    } else {
        "以上为启发式建议，按显存档位推导；实际以「云库」页的流畅/勉强徽章为准。"
    };
    let body = format!(
        "应用：O-deskUI .wokk.（Ollama管理器WokK）v{}\n构建：GTK4 原生（跟随系统主题，SSD 标题栏）\n\n\
         本机硬件：\n{}\n\n\
         本版本更新日志：\n{}\n\n\
         建议模型参数（跟随硬件）：\n• num_ctx（上下文长度）：{ctx}\n• OLLAMA_NUM_PARALLEL（并发）：{parallel}\n• keep_alive（模型常驻）：{keep}\n\n{tip}",
        env!("CARGO_PKG_VERSION"),
        prof.summary(),
        changelog_text()
    );

    let dlg = gtk4::Window::builder()
        .transient_for(win)
        .modal(true)
        .title("关于")
        .default_width(560)
        .default_height(620)
        .build();
    let scroll = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();
    let vbox = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(6)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(20)
        .margin_end(20)
        .build();
    let lbl = gtk4::Label::builder()
        .label(&body)
        .halign(gtk4::Align::Start)
        .valign(gtk4::Align::Start)
        .wrap(true)
        .wrap_mode(gtk4::pango::WrapMode::WordChar)
        .selectable(true)
        .build();
    lbl.set_xalign(0.0);
    vbox.append(&lbl);
    scroll.set_child(Some(&vbox));
    dlg.set_child(Some(&scroll));
    dlg.present();
}
