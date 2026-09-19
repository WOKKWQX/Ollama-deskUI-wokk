//! offideskOllama — GTK4 原生版入口
//! 纯 GTK4 实现，控件外观跟随系统 GTK 主题（Cinnamon / Mint-Y），
//! 原生贴合桌面环境，不使用 libadwaita。

mod config;
mod engine;
mod ollama;
mod registry;
mod rt;
mod service;
mod tray;
mod ui;
mod watch;

use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const APP_ID: &str = "com.offidesk.ollama";

/// 是否以静默模式启动（开机自启动条目带 `--tray` 参数：只进托盘不弹窗）
fn start_hidden() -> bool {
    std::env::args().any(|a| a == "--tray")
}

/// 完全退出前：按设置联动停止 Ollama 引擎（v1.0.21）。
/// 仅本地地址且引擎在运行时执行；同步执行，确保退出前停干净。
fn stop_engine_on_quit() {
    let s = ui::settings::Settings::load();
    if s.stop_engine_on_exit && service::is_local_target() && service::is_running() {
        if let Err(e) = service::stop() {
            eprintln!("[service] 退出联动停止引擎失败: {e}");
        }
    }
}

/// 上次写入菜单栏的引擎运行态，避免无变化时反复重建子菜单
static LAST_ENGINE_MENU_UP: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 空模型库诊断是否已提示过（每次运行只提示一次，避免轮询反复弹窗）
static DIAG_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 重建「操作」子菜单（引擎开关标签随真实状态变化；子菜单与 stop-all 同处一屏）
fn rebuild_engine_menu(em: &gio::Menu, up: bool) {
    em.remove_all();
    em.append(
        Some(if up { "关闭 Ollama 引擎" } else { "打开 Ollama 引擎" }),
        Some("win.engine-toggle"),
    );
    em.append(Some("关闭所有正在运行的模型"), Some("win.stop-all"));
}

fn main() -> glib::ExitCode {
    // 使用系统服务端装饰（SSD）：标题栏交由 Cinnamon / GNOME / Ubuntu 的窗口管理器
    // 绘制，获得原生窗口管理菜单（右键最大化/最小化/关闭等），贴合各桌面环境 UI。
    // 必须在 GTK 初始化前设置（GTK 在首个对象创建时读取该变量）。
    std::env::set_var("GTK_CSD", "0");

    // 初始化纯 GTK4 应用（主题跟随系统 GTK 主题，不引入 libadwaita）
    let app = gtk4::Application::builder()
        .application_id(APP_ID)
        .build();

    // v3.5.2：注册 --tray 选项并接管命令行，修复「开机自启动」实际失效。
    // 开机自启动条目写入的 Exec 带 --tray（仅进托盘不弹窗），此前 GtkApplication
    // 因不认识该参数而直接报「未知选项 --tray」退出，导致 v1.0.21 的「开机自启动」
    // 从上线起实际失效（默认关闭故一直没被发现）。注册为已知选项 + 接管 command-line
    // 信号后，GtkApplication 不再把 --tray 当未知参数拒绝；窗口是否弹出仍由
    // start_hidden()（读取 env args 中的 --tray）决定，行为保持不变。
    app.add_main_option(
        "tray",
        glib::Char(0),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "以静默模式启动（仅托盘，不弹主窗口）",
        None,
    );
    app.connect_command_line(|app, _cmdline| {
        // 接管命令行：激活应用并消费全部参数（含 --tray），不让默认选项解析拒绝未知参数。
        app.activate();
        0
    });

    // 应用用户设置的主题模式（跟随系统 / 强制深 / 强制浅）。
    // 必须在 GTK 初始化之后调用，故放在 startup 信号里（app.run() 后 GTK 才就绪）。
    app.connect_startup(|_| {
        crate::ui::settings::apply_theme();
    });

    // 记录唯一主窗口（RefCell 避免循环引用；仅存一次）
    let window = Rc::new(RefCell::new(None::<gtk4::ApplicationWindow>));

    // activate：单实例——已存在则显示聚焦，否则创建
    let window_clone = Rc::clone(&window);
    app.connect_activate(move |app| {
        let mut w = window_clone.borrow_mut();
        if w.is_none() {
            let win = build_ui(app);
            w.replace(win);
        } else {
            if let Some(win) = w.as_ref() {
                win.present();
                win.unminimize();
            }
        }
    });

    app.run()
}

/// 构建主窗口
/// 使用 gtk4::ApplicationWindow（而非 adw::ApplicationWindow）：
/// 标题栏交由系统窗口管理器绘制（SSD），获得 Cinnamon/GNOME 原生窗口管理菜单。
/// 纯 GTK4 控件，外观跟随系统 GTK 主题。
fn build_ui(app: &gtk4::Application) -> gtk4::ApplicationWindow {
    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .title("O-deskUI .wokk.")
        .icon_name("offideskollama")
        .default_width(1280)
        .default_height(800)
        .build();
    let win: gtk4::Window = window.clone().upcast();

    // ---- 顶部工具栏（含 Ollama 启停按钮）----
    // 窗口使用系统装饰（SSD），标题栏由 Cinnamon 绘制。
    // 此处用普通 Box 作应用工具栏，不再用 HeaderBar（否则出现第二个标题栏）。
    let toolbar = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .build();

    // ---- Ollama 服务启停 ----
    // 控件语义：按钮只表达「将要执行的动作」（启动服务 / 停止服务），
    // 运行状态由状态灯与下方周期轮询真实反映。
    //
    // 修复说明：原实现用 ToggleButton 且把状态与动作混在同一标签
    // （"Ollama 运行中 · 点击停止"），再由 `!btn.is_active()` 反推动作。
    // 但 GTK 的 ToggleButton 在 `clicked` 触发时 active **已经翻转**，
    // 于是「想启动」实际执行 stop、「想停止」实际执行 start —— 两个方向都无效。
    // 此外 systemctl 需要 root，普通用户必然失败且失败被静默吞掉。
    // 现改为：读取真实状态 → 执行相反动作 → 回读校验 → 失败显式报错。
    let svc_btn = gtk4::Button::builder()
        .label("启动服务")
        .icon_name("media-playback-start-symbolic")
        .tooltip_text("启动 / 停止本地 Ollama 服务")
        .build();
    svc_btn.set_sensitive(false); // 首次探测完成前禁用，避免对未知状态误操作
    // 0.17.0：服务启停按钮不再放顶栏，移入「服务」模块页（见下方 svc_page）

    // ---- 连接状态灯 + Ollama 版本 ----
    let status_lbl = gtk4::Label::builder()
        .label("● 检测中…")
        .css_classes(vec!["dim-label"])
        .margin_start(6)
        .build();
    // 0.17.0：连接状态灯同样移入「服务」模块页；状态栏另有一份简洁状态

    // ---- 底部状态栏（稍后挂到主布局底部）——实时反映本地真实状态 ----
    let statusbar = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(16)
        .margin_top(4)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .build();
    let sb_conn = gtk4::Label::builder()
        .label("● 检测中…")
        .css_classes(vec!["dim-label"])
        .halign(gtk4::Align::Start)
        .build();
    let sb_loaded = gtk4::Label::builder()
        .label("已加载：—")
        .css_classes(vec!["dim-label"])
        .halign(gtk4::Align::Start)
        .build();
    let sb_vram = gtk4::Label::builder()
        .label("显存占用：—")
        .css_classes(vec!["dim-label"])
        .halign(gtk4::Align::Start)
        .build();
    let sb_models = gtk4::Label::builder()
        .label("模型：—")
        .css_classes(vec!["dim-label"])
        .halign(gtk4::Align::Start)
        .build();
    statusbar.append(&sb_conn);
    statusbar.append(&sb_loaded);
    statusbar.append(&sb_vram);
    statusbar.append(&sb_models);

    // 真实运行状态的共享镜像（由轮询刷新），点击时据此决定动作
    let running = Arc::new(AtomicBool::new(false));
    let busy = Arc::new(AtomicBool::new(false));

    // v1.0.19：菜单栏「操作」子菜单（引擎开/关标签随真实状态重建）。
    // 由 build_ui 内的轮询闭包直接捕获并重建，无需跨线程全局槽。
    let engine_menu = gio::Menu::new();
    rebuild_engine_menu(&engine_menu, false);

    // v1.0.8：远程/本地分区提示（服务页，随轮询更新可见性与文案）
    let remote_hint = gtk4::Label::builder()
        .css_classes(vec!["dim-label"])
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    remote_hint.set_visible(false);
    remote_hint.set_margin_top(6);

    // 刷新一次真实状态：服务是否运行 + Ollama 版本 + 已加载模型/显存
    let refresh: Rc<dyn Fn()> = {
        let btn = svc_btn.clone();
        let st = status_lbl.clone();
        let rh = remote_hint.clone();
        let engine_menu = engine_menu.clone();
        let running = running.clone();
        let busy = busy.clone();
        let sb_loaded = sb_loaded.clone();
        let sb_vram = sb_vram.clone();
        let sb_conn = sb_conn.clone();
        let sb_models = sb_models.clone();
        let win_poll = window.clone().upcast::<gtk4::Window>();
        Rc::new(move || {
            // 重入保护：上一轮状态查询未返回时本轮直接跳过，
            // 防止 Ollama 忙/网络慢时任务与资源不断堆积（v1.0.4 卡死修复）
            static POLL_BUSY: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if POLL_BUSY.swap(true, Ordering::SeqCst) {
                return;
            }
            let btn2 = btn.clone();
            let st2 = st.clone();
            let remote2 = rh.clone();
            let engine_menu2 = engine_menu.clone();
            let running2 = running.clone();
            let busy2 = busy.clone();
            let sb_loaded2 = sb_loaded.clone();
            let sb_vram2 = sb_vram.clone();
            let sb_conn2 = sb_conn.clone();
            let sb_models2 = sb_models.clone();
            let win_diag = win_poll.clone();
            rt::spawn(
                async move {
                    // service::is_running() 为纯同步实现，可安全地在 future 内调用
                    let up = service::is_running();
                    let managed = service::managed_by().to_string();
                    let ver = ollama::chat::get_version(&ollama::client()).await.ok();
                    let loaded = ollama::models::loaded_models(&ollama::client())
                        .await
                        .unwrap_or_default();
                    let total = ollama::models::list_models(&ollama::client())
                        .await
                        .map(|m| m.len())
                        .unwrap_or(0);
                    (up, managed, ver, loaded, total)
                },
                move |(up, managed, ver, loaded, total)| {
                    POLL_BUSY.store(false, Ordering::SeqCst);
                    running2.store(up, Ordering::SeqCst);
                    // v1.0.8：远程/本地语义分区——状态如实标注目标地址，启停仅本地可用
                    let local = service::is_local_target();
                    let target = crate::config::base_url();
                    let note = if local {
                        String::new()
                    } else {
                        format!("（远程 {target}）")
                    };
                    if local {
                        remote2.set_visible(false);
                    } else {
                        remote2.set_visible(true);
                        remote2.set_text(&format!(
                            "当前地址指向远程机器（{target}）：上方状态为该远程服务的真实可达性；\
                             「启动/停止」只能操作本机服务，已停用。"
                        ));
                    }
                    // 启停操作进行中不抢按钮控制权
                    if !busy2.load(Ordering::SeqCst) {
                        if !local {
                            btn2.set_label("仅管理本机");
                            btn2.set_icon_name("");
                            btn2.set_sensitive(false);
                            btn2.set_tooltip_text(Some(
                                "当前地址指向远程机器，本机服务启停已停用",
                            ));
                        } else if up {
                            btn2.set_label("停止服务");
                            btn2.set_icon_name("media-playback-stop-symbolic");
                            btn2.set_sensitive(true);
                        } else {
                            btn2.set_label("启动服务");
                            btn2.set_icon_name("media-playback-start-symbolic");
                            btn2.set_sensitive(true);
                        }
                        // v1.0.19：同步菜单栏「操作」的引擎开关标签（仅在本地可操作时，
                        // 且运行态翻转时才重建，避免每 3 秒轮询都 remove_all+append 导致菜单闪烁）
                        if local && LAST_ENGINE_MENU_UP.swap(up, Ordering::SeqCst) != up {
                            rebuild_engine_menu(&engine_menu2, up);
                        }
                    }
                    // v3.2.0：同步回写托盘状态（菜单状态行 + 悬浮 tooltip 用）
                    // v3.6.1 修复（托盘「运行 Ollama 引擎」勾不上）：
                    // 勾选态只认「服务是否在运行」（up），不再叠加 ver.is_some()。
                    // 旧实现用 up && ver.is_some()——服务刚起来、版本请求尚未返回
                    // 或超时（/api/version 单独失败）时勾选态为 false，用户在托盘里
                    // 看不到「已运行」，点击又被 start() 的 is_running() 判为已运行
                    // 直接 return，于是表现为「运行欧拉玛失效」。
                    // 版本号只影响摘要文案，不应支配运行态。
                    tray::update_status(
                        up,
                        match (&ver, total) {
                            (Some(v), _) => format!(
                                "Ollama {v} · 本地模型 {total} 个 · 已加载 {} 个",
                                loaded.len()
                            ),
                            (None, 0) => "Ollama 已运行（模型列表为空）".to_string(),
                            (None, n) => {
                                format!("Ollama 已运行 · 本地模型 {n} 个")
                            }
                        },
                    );

                    match ver {
                        Some(v) => {
                            st2.set_label(&format!("● 已连接 Ollama {v}{note}"));
                            st2.remove_css_class("error");
                            st2.add_css_class("success");
                        }
                        None => {
                            st2.set_label(&format!("● 未连接{note}"));
                            st2.remove_css_class("success");
                            st2.add_css_class("error");
                        }
                    }

                    // 底部状态栏：如实展示已加载模型数与显存占用
                    if loaded.is_empty() {
                        sb_loaded2.set_label("已加载：无");
                        sb_vram2.set_label("显存占用：0 B");
                    } else {
                        let vram: u64 = loaded.iter().map(|m| m.size_vram).sum();
                        sb_loaded2.set_label(&format!("已加载：{} 个模型", loaded.len()));
                        sb_vram2
                            .set_label(&format!("显存占用：{}", crate::ollama::human_size(vram)));
                    }
                    // 状态栏左侧：服务方式 + 连接；右侧：模型总数
                    if up {
                        sb_conn2.set_label(&format!("● 服务运行 ({managed})"));
                        sb_conn2.remove_css_class("error");
                        sb_conn2.add_css_class("success");
                    } else {
                        sb_conn2.set_label("● 服务未运行");
                        sb_conn2.remove_css_class("success");
                        sb_conn2.add_css_class("error");
                    }
                    sb_models2.set_label(&format!("模型：{total} 个"));
                    // v3.6.1：服务在跑但模型为 0 时，主动诊断是否「服务读错目录」。
                    // 只提示一次（DIAG_SHOWN），避免每 3 秒轮询反复弹窗打扰。
                    if up && total == 0 && !DIAG_SHOWN.swap(true, Ordering::SeqCst) {
                        if let Some(msg) = crate::engine::diagnose_empty_model_dir() {
                            sb_models2.set_label("模型：0 个（可能读取了错误的目录）");
                            sb_models2.remove_css_class("success");
                            sb_models2.add_css_class("error");
                            ui::dialogs::error(
                                &win_diag,
                                "模型列表为空，但磁盘上有模型",
                                &msg,
                            );
                        }
                    }
                },
            );
        })
    };

    // v1.0.19：引擎启停共享动作（服务按钮与菜单栏「操作」共用，避免重复逻辑）
    let engine_toggle_action: Rc<dyn Fn()> = {
        let win_err = win.clone();
        let running = running.clone();
        let busy = busy.clone();
        let refresh2 = refresh.clone();
        Rc::new(move || {
            if busy.load(Ordering::SeqCst) {
                return;
            }
            // 远程地址守卫：启停只作用于本机服务
            if !service::is_local_target() {
                ui::dialogs::error(
                    &win_err,
                    "无法启停服务",
                    "当前 Ollama 服务地址指向远程机器，\n「启动/停止服务」只能操作本机服务，对远程地址无效。\n\n\
                     如需管理本机服务，请先到 设置 → Ollama 服务地址 改回 http://localhost:11434。",
                );
                return;
            }
            busy.store(true, Ordering::SeqCst);
            let want_start = !running.load(Ordering::SeqCst);
            let busy2 = busy.clone();
            let refresh2 = refresh2.clone();
            let win_err2 = win_err.clone();
            rt::spawn(
                async move {
                    if want_start {
                        service::start()
                    } else {
                        service::stop()
                    }
                },
                move |res: Result<(), String>| {
                    busy2.store(false, Ordering::SeqCst);
                    if let Err(e) = res {
                        eprintln!("[service] 启停失败: {e}");
                        ui::dialogs::error(
                            &win_err2,
                            if want_start { "启动失败" } else { "停止失败" },
                            &e,
                        );
                        ui::dialogs::notify(
                            &win_err2,
                            if want_start { "启动失败" } else { "停止失败" },
                            &e,
                        );
                    }
                    refresh2(); // 回读真实状态，校正按钮与菜单标签
                },
            );
        })
    };

    // 立即刷新一次，之后每 3 秒同步（满足「全场景实时反映本地真实状态」）
    refresh();
    {
        let r = refresh.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(3), move || {
            r();
            glib::ControlFlow::Continue
        });
    }

    // v1.0.21：按设置在应用启动时自动拉起引擎（仅本地、仅在未运行时；
    // 后台线程执行避免 systemctl/pkexec 阻塞主循环，状态由 3 秒轮询如实反映）
    {
        let s = ui::settings::Settings::load();
        if s.start_engine_on_launch && service::is_local_target() && !service::is_running() {
            std::thread::spawn(|| {
                if let Err(e) = service::start() {
                    eprintln!("[service] 应用启动时自动拉起引擎失败: {e}");
                }
            });
        }
    }

    // 点击启停：复用共享引擎动作（v1.0.19 起与菜单栏「操作」同一实现）
    {
        let act = engine_toggle_action.clone();
        svc_btn.connect_clicked(move |b| {
            b.set_sensitive(false);
            b.set_label("处理中…");
            act();
        });
    }

    // v1.0.16：顶栏右侧不再放 设置/关于 按钮——设置只留菜单一个入口，
    // 更新日志收敛进设置窗内，消除「三个设置入口」的困惑。
    let spacer = gtk4::Box::builder().orientation(gtk4::Orientation::Horizontal).hexpand(true).build();
    toolbar.append(&spacer);

    // ---- 主内容：左侧模块导航 + 右侧模块主区（v1.0.16 IA 重构）----
    // 8 项导航收敛为 4 项：概览(含服务) / 模型管理 / 模型云库 / 运行测试(对话·生成·嵌入)
    let (chat_widget, chat_pane) = ui::chat::build_chat_pane(&win);
    let (gen_widget, gen_panel) = ui::panels::build_generate_panel(&win);
    // 已加载面板已随 1.0.20 删除：其 1.0.16 起职能由概览页承担，但残留构造仍会
    // 挂载轮询定时器——且该面板的 schedule 存在「每次触发新建定时器 + 自身
    // Continue 续期」的指数倍增缺陷，正是长期未决「启动数分钟后必卡死」的元凶。
    let (embed_widget, embed_panel) = ui::panels::build_embed_panel();

    // 选中模型：联动到聊天 / 生成 / 嵌入 三个面板
    let chat_pane_for_select = chat_pane.clone();
    let on_model_selected: Rc<dyn Fn(String)> = Rc::new(move |model: String| {
        let m = model.clone();
        {
            let pane = chat_pane_for_select.borrow();
            ui::chat::set_model(&pane, &m);
        }
        {
            let p = gen_panel.borrow();
            p.set_model(&m);
        }
        {
            let p = embed_panel.borrow();
            p.set_model(&m);
        }
    });

    // 模块主区（纯 GTK4 Stack；导航由左侧 ListBox 承担，不再用底部 StackSwitcher）
    let stack = gtk4::Stack::builder().hexpand(true).vexpand(true).build();

    // ① 概览 · 驾驶舱（服务控制稍后并入，见 svc_page 组装处）
    let overview_widget = ui::overview::build_overview_page(&win);

    // ② 模型管理 / ③ 模型云库：拆为两个独立页面（v1.0.16）——
    // 原先 Paned 并排把「管理本地模型」与「浏览云库」混在一屏，互相干扰；
    // 模型管理是本应用的主功能，独占一页；云库是其中的重点延伸功能，单独一页。
    let (sidebar_widget, sidebar_state) = ui::sidebar::build_sidebar(
        &win,
        {
            let cb = on_model_selected.clone();
            move |s: String| cb.clone()(s)
        },
    );
    stack.add_titled(&sidebar_widget, Some("models_mgr"), "管理");
    let discover_widget =
        ui::discover::build_discover_page(&win, &sidebar_state, on_model_selected.clone());
    stack.add_titled(&discover_widget, Some("cloud"), "云库");

    // ③ 对话模块（v3.1 重设计）：会话管理移入对话页左侧 ListBox 侧栏
    // （新建=侧栏头部按钮，切换=点击行，删除=行内图标），原顶部会话操作行整体移除；
    // 清空/导出/重新生成/参数在对话面板操作行与顶栏，入口唯一。
    let chat_module = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .build();
    chat_module.append(&chat_widget);
    // v3.0.1 修复：chat_module 只归「测试」页 run_stack 挂载。此前多出的
    // stack.add_titled(&chat_module, "chat") 让同一部件被两个 Stack 争抢，
    // GTK4 拒绝二次挂载，导致「测试→对话」页内容区整片空白。
    // 主导航仅 概览/管理/云库/测试 四项，无任何代码引用 "chat" 页名，删除无副作用。

    // ④ 运行测试：对话 / 生成 / 嵌入 三合一，页内 StackSwitcher 切换（v1.0.16）
    let run_stack = gtk4::Stack::builder().vexpand(true).hexpand(true).build();
    run_stack.add_titled(&chat_module, Some("chat"), "对话");
    run_stack.add_titled(&gen_widget, Some("generate"), "生成");
    run_stack.add_titled(&embed_widget, Some("embed"), "嵌入");
    let run_switcher = gtk4::StackSwitcher::builder().stack(&run_stack).build();
    run_switcher.set_halign(gtk4::Align::Center);
    run_switcher.set_margin_top(8);
    let run_page = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .build();
    run_page.append(&run_switcher);
    run_page.append(&run_stack);
    stack.add_titled(&run_page, Some("run"), "测试");

    // ⑤ 服务控制并入概览（v1.0.16）：服务启停本就服务于「当前状态」，与驾驶舱同屏
    let svc_page = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .margin_top(8)
        .margin_bottom(16)
        .margin_start(18)
        .margin_end(18)
        .build();
    let svc_title = gtk4::Label::new(None);
    svc_title.set_markup("<span size='large' weight='bold'>服务控制</span>");
    svc_title.set_halign(gtk4::Align::Start);
    svc_page.append(&svc_title);
    let svc_row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(10)
        .build();
    svc_row.append(&svc_btn);
    svc_row.append(&status_lbl);
    svc_page.append(&svc_row);
    // v1.0.8：远程地址时的语义分区说明（本地地址自动隐藏）
    svc_page.append(&remote_hint);
    let svc_log_btn = gtk4::Button::builder().label("查看服务日志").build();
    {
        let w = win.clone();
        svc_log_btn.connect_clicked(move |_| crate::ui::dialogs::server_log(&w));
    }
    svc_page.append(&svc_log_btn);
    let svc_hint = gtk4::Label::builder()
        .label("服务端参数（模型目录 / keep_alive / 并发 / GPU）在 设置 → 服务端配置。\n启停失败时会在对话框中列出依次尝试的步骤与原因。")
        .css_classes(vec!["dim-label"])
        .halign(gtk4::Align::Start)
        .build();
    svc_page.append(&svc_hint);
    // 概览 = 驾驶舱 + 服务控制（v1.0.16），单页归属「概览」
    let overview_page = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .build();
    overview_page.append(&overview_widget);
    overview_page.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    overview_page.append(&svc_page);
    stack.add_titled(&overview_page, Some("overview"), "概览");

    // ---- 左侧导航（概览 / 模型管理 / 模型云库 / 运行测试）——v1.0.16 收敛为 4 项 ----
    // v3.3.0：加 .navigation-sidebar，套用 Cinnamon/Mint-Y 原生侧栏样式
    // （36px 行高、悬停底色、选中态用系统强调色），与系统侧栏观感一致。
    let nav = gtk4::ListBox::builder()
        .selection_mode(gtk4::SelectionMode::Single)
        .css_classes(vec!["navigation-sidebar"])
        .build();
    let nav_names = ["overview", "models_mgr", "cloud", "run"];
    // v3.2.0：导航行加 symbolic 图标（图标+文字横排），与主题选中态联动
    for (label, icon) in [
        ("概览", "view-grid-symbolic"),
        ("管理", "package-x-generic-symbolic"),
        ("云库", "folder-remote-symbolic"),
        ("测试", "media-playback-symbolic"),
    ] {
        let row = gtk4::ListBoxRow::new();
        let hb = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .build();
        let ic = gtk4::Image::from_icon_name(icon);
        let lbl = gtk4::Label::builder().label(label).halign(gtk4::Align::Start).build();
        hb.append(&ic);
        hb.append(&lbl);
        row.set_child(Some(&hb));
        nav.append(&row);
    }
    {
        let stack = stack.clone();
        nav.connect_row_activated(move |list, row| {
            let idx = row.index();
            if let Some(name) = nav_names.get(idx as usize) {
                stack.set_visible_child_name(name);
            }
        });
    }
    let nav_scroll = gtk4::ScrolledWindow::builder()
        .width_request(150)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&nav)
        .build();

    // 默认进入概览
    stack.set_visible_child_name("overview");

    let content = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .vexpand(true)
        .build();
    content.append(&nav_scroll);
    content.append(&stack);

    // ---- 整体布局 ----
    let outer = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .build();

    // ---- 顶部菜单栏（v1.0.19）：原生 PopoverMenuBar，替代原 ☰ 按钮 ----
    // 四个主菜单：查看 / 转到 / 操作 / 帮助，贴合 GNOME/Cinnamon 的菜单栏交互。
    let act_group = gio::SimpleActionGroup::new();
    let add_act = |name: &str, on_activate: Box<dyn Fn()>| {
        let a = gio::SimpleAction::new(name, None);
        a.connect_activate(move |_, _| on_activate());
        act_group.add_action(&a);
    };

    // 查看 → 主题 / 信息
    add_act("theme-system", Box::new(|| ui::settings::set_theme_mode("system")));
    add_act("theme-dark", Box::new(|| ui::settings::set_theme_mode("dark")));
    add_act("theme-light", Box::new(|| ui::settings::set_theme_mode("light")));
    {
        let w = win.clone();
        add_act("show-info", Box::new(move || ui::settings::app_info_dialog(&w)));
    }
    // 转到 → 主页 / 模型管理
    {
        let s = stack.clone();
        add_act("go-home", Box::new(move || s.set_visible_child_name("overview")));
    }
    {
        let s = stack.clone();
        add_act("go-models", Box::new(move || s.set_visible_child_name("models_mgr")));
    }
    // v3.2.0：托盘快捷入口所需的两处跳转
    {
        let s = stack.clone();
        add_act("goto-cloud", Box::new(move || s.set_visible_child_name("cloud")));
    }
    {
        let s = stack.clone();
        add_act("goto-run", Box::new(move || s.set_visible_child_name("run")));
    }
    // 操作 → 引擎开关（标签随真实状态重建，见 engine_menu）/ 关闭所有模型
    {
        let act = engine_toggle_action.clone();
        add_act("engine-toggle", Box::new(move || act()));
    }
    {
        let w = win.clone();
        add_act("stop-all", Box::new(move || {
            let w2 = w.clone();
            let client = ollama::client();
            crate::rt::spawn(
                async move { ollama::models::loaded_models(&client).await },
                move |res: Result<Vec<ollama::LoadedModel>, ollama::OllamaError>| {
                    match res {
                        Ok(ms) if ms.is_empty() => {
                            ui::dialogs::notify(&w2, "无需操作", "当前没有正在运行的模型。");
                        }
                        Ok(ms) => {
                            let total = ms.len();
                            for m in &ms {
                                let c = ollama::client();
                                let nm = m.name.clone();
                                crate::rt::spawn(
                                    async move {
                                        ollama::models::stop_model(&c, &nm).await.map_err(|e| e.to_string())
                                    },
                                    move |r: Result<(), String>| {
                                        if let Err(e) = r {
                                            eprintln!("[stop-all] 卸载失败: {e}");
                                        }
                                    },
                                );
                            }
                            ui::dialogs::notify(
                                &w2,
                                "正在卸载",
                                &format!("已请求卸载 {total} 个运行中的模型。"),
                            );
                        }
                        Err(e) => ui::dialogs::error(&w2, "读取运行模型失败", &e.to_string()),
                    }
                },
            );
        }));
    }
    // 帮助 → 设置 / 服务日志 / 首次使用引导 / 注册中心登录 / 注册中心登出
    {
        let w = win.clone();
        add_act("open-settings", Box::new(move || ui::settings::open_settings(&w)));
    }
    {
        let w = win.clone();
        add_act("server-log", Box::new(move || ui::dialogs::server_log(&w)));
    }
    {
        let w = win.clone();
        add_act("welcome", Box::new(move || ui::settings::open_welcome(&w)));
    }
    {
        let w = win.clone();
        add_act("reg-login", Box::new(move || {
            let w_call = w.clone();
            let w_inner = w.clone();
            ui::dialogs::registry_login(&w_call, move |(reg, token)| {
                crate::registry::save_auth(&reg, &token);
                ui::dialogs::notify(&w_inner, "已登录", &format!("注册中心 {reg} 的鉴权已保存"));
            });
        }));
    }
    {
        let w = win.clone();
        add_act("reg-logout", Box::new(move || {
            let w_call = w.clone();
            let w_inner = w.clone();
            ui::dialogs::registry_logout(&w_call, move |reg| {
                crate::registry::remove_auth(&reg);
                ui::dialogs::notify(&w_inner, "已登出", &format!("注册中心 {reg} 的鉴权已移除"));
            });
        }));
    }
    window.insert_action_group("win", Some(&act_group));

    // 菜单模型（四个主菜单）
    let view_menu = gio::Menu::new();
    let theme_sub = gio::Menu::new();
    theme_sub.append(Some("跟随系统"), Some("win.theme-system"));
    theme_sub.append(Some("深色"), Some("win.theme-dark"));
    theme_sub.append(Some("浅色"), Some("win.theme-light"));
    view_menu.append_submenu(Some("主题"), &theme_sub);
    view_menu.append(Some("信息"), Some("win.show-info"));

    let go_menu = gio::Menu::new();
    go_menu.append(Some("主页"), Some("win.go-home"));
    go_menu.append(Some("模型管理"), Some("win.go-models"));

    // 「操作」子菜单直接复用 engine_menu（含引擎开关 + 关闭所有模型，标签随状态重建）
    let op_menu = engine_menu.clone();

    let help_menu = gio::Menu::new();
    help_menu.append(Some("设置"), Some("win.open-settings"));
    help_menu.append(Some("服务日志"), Some("win.server-log"));
    help_menu.append(Some("首次使用引导"), Some("win.welcome"));
    help_menu.append(Some("注册中心登录"), Some("win.reg-login"));
    help_menu.append(Some("注册中心登出"), Some("win.reg-logout"));

    let menubar_model = gio::Menu::new();
    menubar_model.append_submenu(Some("查看"), &view_menu);
    menubar_model.append_submenu(Some("转到"), &go_menu);
    menubar_model.append_submenu(Some("操作"), &op_menu);
    menubar_model.append_submenu(Some("帮助"), &help_menu);

    // v3.2.0：弃用 PopoverMenuBar 自绘模仿路线，改走 ApplicationWindow 内建
    // 菜单栏（set_show_menubar）。GTK4 唯一保留的「真菜单栏」路径：由主题原生
    // 渲染 menubar 节点，与 Nemo 等系统应用同一绘制方式，视觉随主题走。
    app.set_menubar(Some(&menubar_model));
    window.set_show_menubar(true);

    // 快捷键：Ctrl+, 设置；Ctrl+1~4 切主页面；Ctrl+F 转云库搜索；F5 刷新模型列表
    {
        let key = gtk4::EventControllerKey::new();
        let w = win.clone();
        let stack_kv = stack.clone();
        let chat_pane_kv = chat_pane.clone();
        key.connect_key_pressed(move |_, keyval, _, state| {
            let ctrl = state.contains(gtk4::gdk::ModifierType::CONTROL_MASK);
            if ctrl && keyval == gtk4::gdk::Key::comma {
                crate::ui::settings::open_settings(&w);
                return glib::Propagation::Stop;
            }
            if ctrl {
                let page = match keyval {
                    gtk4::gdk::Key::_1 => Some("overview"),
                    gtk4::gdk::Key::_2 => Some("models_mgr"),
                    gtk4::gdk::Key::_3 => Some("cloud"),
                    gtk4::gdk::Key::_4 => Some("run"),
                    _ => None,
                };
                if let Some(p) = page {
                    stack_kv.set_visible_child_name(p);
                    return glib::Propagation::Stop;
                }
                if keyval == gtk4::gdk::Key::f {
                    stack_kv.set_visible_child_name("cloud");
                    return glib::Propagation::Stop;
                }
            }
            if keyval == gtk4::gdk::Key::F5 {
                crate::ui::chat::refresh_model_combo(&chat_pane_kv);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        win.add_controller(key);
    }

    outer.append(&toolbar);
    outer.append(&content);
    outer.append(&statusbar);

    window.set_child(Some(&outer));
    // v1.0.21：开机自启动以 `--tray` 参数启动——只进托盘，不弹主窗口
    if !start_hidden() {
        window.present();
    }

    // ---- 托盘是否成功启动（跨线程共享）----
    let tray_ok = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let tray_ok_for_close = tray_ok.clone();

    // ---- 关闭窗口：若设置"关闭到托盘"且托盘成功 → 隐藏；否则正常退出 ----
    let w_btn = window.clone();
    let app_close = app.clone();
    let tray_ok_close = tray_ok_for_close.clone();
    window.connect_close_request(move |_| {
        let close_to_tray = ui::settings::Settings::load().close_to_tray;
        let tray_ok = tray_ok_close.load(std::sync::atomic::Ordering::SeqCst);

        // ---- 退出守门：有进行中的任务时，列出将中断的任务再确认（0.17.0）----
        // 注意：全程 try_borrow，绝不与流式回调的 borrow_mut 冲突（否则回调期间
        // 关窗会因借用冲突 panic 闪退）。
        let streaming = chat_pane
            .try_borrow()
            .ok()
            .and_then(|p| p.streaming.try_borrow().ok().map(|v| *v))
            .unwrap_or(false);
        let pulling = ui::sidebar::PULL_ACTIVE.load(std::sync::atomic::Ordering::SeqCst);
        if streaming || pulling {
            let mut tasks = String::new();
            if streaming {
                tasks.push_str("● 正在生成回复（对话进行中）\n");
            }
            if pulling {
                tasks.push_str("● 正在拉取模型（下载未完成）\n");
            }
            let dlg = gtk4::MessageDialog::builder()
                .transient_for(&w_btn)
                .modal(true)
                .text("退出 O-deskUI .wokk.？")
                .secondary_text(&format!("以下任务将被中断：\n{tasks}\n建议最小化到托盘，回来即续。"))
                .buttons(gtk4::ButtonsType::None)
                .build();
            if tray_ok {
                let tray_btn = dlg.add_button("最小化到托盘", gtk4::ResponseType::Apply);
                tray_btn.set_css_classes(&["suggested-action"]);
            }
            dlg.add_button("仍要退出", gtk4::ResponseType::Yes);
            dlg.add_button("取消", gtk4::ResponseType::Cancel);
            let app_guard = app_close.clone();
            let w_guard = w_btn.clone();
            dlg.connect_response(move |d, resp| {
                match resp {
                    gtk4::ResponseType::Apply => {
                        d.close();
                        w_guard.hide(); // 最小化到托盘，任务继续
                    }
                    gtk4::ResponseType::Yes => {
                        d.close();
                        stop_engine_on_quit(); // v1.0.21：按设置联动停止引擎
                        app_guard.quit();
                    }
                    _ => d.close(),
                }
            });
            dlg.present();
            return glib::Propagation::Stop;
        }

        if close_to_tray && tray_ok {
            // 隐藏窗口，进程驻留（托盘常驻）
            w_btn.hide();
            glib::Propagation::Stop
        } else {
            // 未启用托盘或托盘失败：真正退出
            app_close.quit();
            glib::Propagation::Proceed
        }
    });

    // ---- 启动系统托盘（ksni，纯 Rust SNI）----
    let (ttx, trx) = std::sync::mpsc::channel::<tray::TrayCmd>();
    let (tok_tx, tok_rx) = std::sync::mpsc::channel::<bool>();
    std::thread::spawn(move || {
        crate::tray::start_tray(ttx, tok_tx);
    });
    // 主线程轮询：托盘启动结果 + 托盘命令
    let tray_ok_task = tray_ok.clone();
    let tw = window.clone();
    let app_tray = app.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
        // 处理托盘启动结果
        while let Ok(ok) = tok_rx.try_recv() {
            tray_ok_task.store(ok, std::sync::atomic::Ordering::SeqCst);
            if ok {
                eprintln!("[tray] 托盘已启动");
            }
        }
        // 处理托盘命令
        match trx.try_recv() {
            Ok(tray::TrayCmd::Show) => {
                tw.set_visible(true);
                tw.present();
                tw.unminimize();
            }
            Ok(tray::TrayCmd::Hide) => {
                tw.hide();
            }
            Ok(tray::TrayCmd::EngineToggle) => {
                gio::prelude::ActionGroupExt::activate_action(&tw, "engine-toggle", None);
            }
            Ok(tray::TrayCmd::GotoCloud) => {
                gio::prelude::ActionGroupExt::activate_action(&tw, "goto-cloud", None);
                tw.set_visible(true);
                tw.present();
                tw.unminimize();
            }
            Ok(tray::TrayCmd::OpenDocs) => {
                ui::settings::open_docs(tw.upcast_ref());
                tw.set_visible(true);
                tw.present();
            }
            Ok(tray::TrayCmd::Quit) => {
                stop_engine_on_quit(); // v1.0.21：按设置联动停止引擎
                app_tray.quit();
                return glib::ControlFlow::Break;
            }
            Err(_) => {}
        }
        glib::ControlFlow::Continue
    });

    // ---- 卡死黑匣子（v1.0.12）：主循环心跳 + 看门狗 ----
    // 卡死时自动把各线程阻塞点写入 ~/.cache/offideskollama/watchdog.log
    crate::watch::start();

    // ---- 首次使用引导：仅当 settings.json 不存在（真正首次）时弹出 ----
    let first_run = !ui::settings::Settings::load_path().exists();
    if first_run && !start_hidden() {
        // 静默启动（开机自启）不弹首次引导，用户点托盘进窗口时仍可见界面
        ui::settings::open_welcome(&win);
    }

    window
}