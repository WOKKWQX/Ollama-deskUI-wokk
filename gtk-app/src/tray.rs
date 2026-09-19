//! 系统托盘（XDG StatusNotifierItem，经 ksni 实现，纯 Rust 无 GTK 依赖）
//! 托盘菜单：显示窗口 / 隐藏到托盘 / 退出；左键点击托盘显示窗口。
//! 托盘回调运行在 ksni 内部线程，通过 mpsc 把命令送回 GTK 主线程执行。
//! 图标优先用 icon_pixmap（从已安装应用图标读取 RGBA），保证 SNI 在 Cinnamon 等
//! 环境始终有可见图标（icon_name 在未更新图标缓存时可能解析不到，导致托盘空白/点不动）。

use ksni::menu::{CheckmarkItem, MenuItem, StandardItem};
use ksni::{Handle, Icon, ToolTip, Tray, TrayMethods};
use std::sync::mpsc::Sender;
use std::sync::Mutex;

/// 托盘命令（送回 GTK 主线程）
#[derive(Debug, Clone, Copy)]
pub enum TrayCmd {
    Show,
    Hide,
    Quit,
    /// 切换 Ollama 引擎启停
    EngineToggle,
    /// 打开「云库」页
    GotoCloud,
    /// 打开教学文档
    OpenDocs,
}

/// 托盘展示的实时状态（GTK 主循环周期性回写；ksni 线程只读）
static STATUS: Mutex<(bool, String)> = Mutex::new((false, String::new()));

/// GTK 主线程回写托盘状态（引擎是否运行 + 摘要行）
pub fn update_status(engine_running: bool, summary: String) {
    if let Ok(mut st) = STATUS.lock() {
        st.0 = engine_running;
        st.1 = summary;
    }
}

fn status_snapshot() -> (bool, String) {
    STATUS.lock().map(|s| s.clone()).unwrap_or((false, String::new()))
}

/// 尝试从已安装的应用图标读取像素图；读不到则回退为纯色方块（保证托盘始终可见）
fn load_pixmap() -> Vec<Icon> {
    let candidates = [
        "/usr/share/icons/hicolor/256x256/apps/offideskollama.png",
        "/usr/share/icons/hicolor/128x128/apps/offideskollama.png",
        "/usr/share/icons/hicolor/64x64/apps/offideskollama.png",
    ];
    for p in candidates {
        if let Ok(pb) = gdk_pixbuf::Pixbuf::from_file(p) {
            let w = pb.width();
            let h = pb.height();
            let stride = pb.rowstride() as usize;
            let px = unsafe { pb.pixels() };
            let bytes: &[u8] = &px;
            let mut data = Vec::with_capacity((w * h * 4) as usize);
            for y in 0..(h as usize) {
                let start = y * stride;
                data.extend_from_slice(&bytes[start..start + (w as usize) * 4]);
            }
            if !data.is_empty() && data.len() == (w * h * 4) as usize {
                return vec![Icon { width: w, height: h, data }];
            }
        }
    }
    // 回退：品牌色方块（#3366cc）
    let s = 64i32;
    let mut data = Vec::with_capacity((s * s * 4) as usize);
    for _ in 0..(s * s) {
        data.extend_from_slice(&[0x33, 0x66, 0xcc, 0xff]);
    }
    vec![Icon { width: s, height: s, data }]
}

/// 实现 ksni::Tray 的托盘状态
struct DeskTray {
    tx: Sender<TrayCmd>,
}

impl Tray for DeskTray {
    fn id(&self) -> String {
        "offideskollama".into()
    }
    fn title(&self) -> String {
        "O-deskUI .wokk.".into()
    }
    fn icon_name(&self) -> String {
        "offideskollama".into() // 应用自有图标（hlcolor 主题中已注册）
    }
    fn icon_pixmap(&self) -> Vec<Icon> {
        load_pixmap()
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let (engine_on, summary) = status_snapshot();
        let txy_show = self.tx.clone();
        let txy_hide = self.tx.clone();
        let txy_cloud = self.tx.clone();
        let txy_docs = self.tx.clone();
        let txy_engine = self.tx.clone();
        let txy_quit = self.tx.clone();
        vec![
            // 顶部状态行（不可点击，只读信息）
            StandardItem {
                label: if summary.is_empty() {
                    "状态读取中…".into()
                } else {
                    summary
                },
                enabled: false,
                visible: true,
                icon_name: if engine_on {
                    "media-playback-start-symbolic".into()
                } else {
                    "media-playback-stop-symbolic".into()
                },
                icon_data: Vec::new(),
                shortcut: Vec::new(),
                disposition: ksni::menu::Disposition::Normal,
                activate: Box::new(|_| {}),
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "显示主窗口".into(),
                enabled: true,
                visible: true,
                icon_name: "view-restore-symbolic".into(),
                icon_data: Vec::new(),
                shortcut: Vec::new(),
                disposition: ksni::menu::Disposition::Normal,
                activate: Box::new(move |_| {
                    let _ = txy_show.send(TrayCmd::Show);
                }),
            }
            .into(),
            StandardItem {
                label: "隐藏到托盘".into(),
                enabled: true,
                visible: true,
                icon_name: "view-fullscreen-symbolic".into(),
                icon_data: Vec::new(),
                shortcut: Vec::new(),
                disposition: ksni::menu::Disposition::Normal,
                activate: Box::new(move |_| {
                    let _ = txy_hide.send(TrayCmd::Hide);
                }),
            }
            .into(),
            MenuItem::Separator,
            // 引擎勾选项：勾选态 = 引擎运行中，点击即切换
            CheckmarkItem {
                label: "运行 Ollama 引擎".into(),
                enabled: true,
                visible: true,
                checked: engine_on,
                icon_name: String::new(),
                icon_data: Vec::new(),
                shortcut: Vec::new(),
                disposition: ksni::menu::Disposition::Normal,
                activate: Box::new(move |_| {
                    let _ = txy_engine.send(TrayCmd::EngineToggle);
                }),
            }
            .into(),
            StandardItem {
                label: "打开云库".into(),
                enabled: true,
                visible: true,
                icon_name: "folder-remote-symbolic".into(),
                icon_data: Vec::new(),
                shortcut: Vec::new(),
                disposition: ksni::menu::Disposition::Normal,
                activate: Box::new(move |_| {
                    let _ = txy_cloud.send(TrayCmd::GotoCloud);
                }),
            }
            .into(),
            StandardItem {
                label: "教学文档".into(),
                enabled: true,
                visible: true,
                icon_name: "help-contents-symbolic".into(),
                icon_data: Vec::new(),
                shortcut: Vec::new(),
                disposition: ksni::menu::Disposition::Normal,
                activate: Box::new(move |_| {
                    let _ = txy_docs.send(TrayCmd::OpenDocs);
                }),
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "退出 O-deskUI .wokk.".into(),
                enabled: true,
                visible: true,
                icon_name: "application-exit-symbolic".into(),
                icon_data: Vec::new(),
                shortcut: Vec::new(),
                disposition: ksni::menu::Disposition::Normal,
                activate: Box::new(move |_| {
                    let _ = txy_quit.send(TrayCmd::Quit);
                }),
            }
            .into(),
        ]
    }

    // 悬浮提示（SNI tooltip）：引擎状态 + 版本摘要
    fn tool_tip(&self) -> ToolTip {
        let (engine_on, summary) = status_snapshot();
        ToolTip {
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
            title: "O-deskUI .wokk.".into(),
            description: if engine_on {
                format!("引擎运行中\n{}", summary)
            } else {
                format!("引擎未运行\n{}", summary)
            },
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(TrayCmd::Show);
    }
}

/// 启动系统托盘。`ok_tx` 用于在启动成功/失败后向主线程回传结果。
pub fn start_tray(tx: Sender<TrayCmd>, ok_tx: std::sync::mpsc::Sender<bool>) {
    let tray = DeskTray { tx };
    // 关键修复：桌面 SNI 宿主（Cinnamon / Mint 等）在程序启动时常尚未就绪，
    // `service::run` 会返回 Err(WontShow / Watcher)，导致 spawn 立即失败、
    // `tray_ok` 永远为 false、点关闭窗口直接退出而非最小化到托盘（即"托盘控制无效"）。
    // `assume_sni_available(true)` 把这些错误转成「watcher 离线后重连」的软错误，
    // spawn 正常返回 Ok(Handle) → 托盘真正可用，关闭窗口才能最小化到托盘。
    match crate::rt::block_on(tray.assume_sni_available(true).spawn()) {
        Ok(h) => {
            // 保活：把 handle 存进静态，防止被 drop（drop 后托盘消失）
            static HOLD: std::sync::OnceLock<std::sync::Mutex<Option<Handle<DeskTray>>>> =
                std::sync::OnceLock::new();
            let hold = HOLD.get_or_init(|| std::sync::Mutex::new(None));
            *hold.lock().unwrap_or_else(|e| e.into_inner()) = Some(h);
            let _ = ok_tx.send(true);
        }
        Err(e) => {
            eprintln!("[tray] 启动托盘失败: {e}");
            let _ = ok_tx.send(false);
        }
    }
}
