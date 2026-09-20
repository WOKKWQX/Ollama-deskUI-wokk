//! 通用对话框：确认框 / 文本输入框 / 信息框 / 进度框
//! 纯 GTK4 实现（自定义 Window 对话框，不依赖 libadwaita / AlertDialog，
//! 从而无需抬高 GTK 特性版本要求，在较旧的 GTK4 运行时也能工作）。
//! 所有函数接收父窗口做 transient_for。

use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 通用选择对话框：标题 + 正文 + 一组按钮，按按钮 id 回调。
/// buttons: (标签, id, css_class)，css_class 为空串 / "suggested-action" / "destructive-action"。
fn choice_dialog(
    parent: &gtk4::Window,
    title: &str,
    body: &str,
    buttons: &[(&str, &str, &str)],
    on_resp: Rc<dyn Fn(&str)>,
) {
    let win = gtk4::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(title)
        .default_width(440)
        .build();

    let title_lbl = gtk4::Label::builder()
        .label(title)
        .halign(gtk4::Align::Start)
        .wrap(true)
        .css_classes(vec!["title-4", "heading"])
        .build();
    let body_lbl = gtk4::Label::builder()
        .label(body)
        .halign(gtk4::Align::Start)
        .wrap(true)
        .css_classes(vec!["dim-label"])
        .build();

    let vbox = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    vbox.append(&title_lbl);
    vbox.append(&body_lbl);

    let bar = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk4::Align::End)
        .build();
    let win_c = win.clone();
    for (label, id, cls) in buttons {
        let btn = gtk4::Button::builder().label(*label).build();
        if !cls.is_empty() {
            btn.set_css_classes(&[cls]);
        }
        let id = id.to_string();
        let on_resp = on_resp.clone();
        let win_cc = win_c.clone();
        btn.connect_clicked(move |_| {
            on_resp(&id);
            win_cc.close();
        });
        bar.append(&btn);
    }
    vbox.append(&bar);
    win.set_child(Some(&vbox));
    win.present();
}

/// 确认框
pub fn confirm(
    parent: &gtk4::Window,
    title: &str,
    body: &str,
    confirm_label: &str,
    on_confirm: impl Fn() + 'static,
) {
    let on_confirm = Rc::new(on_confirm);
    let on_resp = Rc::new(move |id: &str| {
        if id == "ok" {
            on_confirm();
        }
    });
    choice_dialog(
        parent,
        title,
        body,
        &[("取消", "cancel", ""), (confirm_label, "ok", "destructive-action")],
        on_resp,
    );
}

/// 主操作确认框（确认按钮用 suggested-action）。
/// 与 `confirm` 的区别在语义：`confirm` 用于删除等破坏性操作，
/// 本函数用于「开始下载」这类正向但需要二次确认的动作。
pub fn confirm_primary(
    parent: &gtk4::Window,
    title: &str,
    body: &str,
    confirm_label: &str,
    on_confirm: impl Fn() + 'static,
) {
    let on_confirm = Rc::new(on_confirm);
    let on_resp = Rc::new(move |id: &str| {
        if id == "ok" {
            on_confirm();
        }
    });
    choice_dialog(
        parent,
        title,
        body,
        &[("取消", "cancel", ""), (confirm_label, "ok", "suggested-action")],
        on_resp,
    );
}

/// 错误框。启停服务等操作失败时必须显式告知用户原因，
/// 不能静默失败（历史上「启停按键无效」正是失败被吞掉导致的）。
pub fn error(parent: &gtk4::Window, title: &str, body: &str) {
    let on_resp: Rc<dyn Fn(&str)> = Rc::new(|_| {});
    choice_dialog(parent, title, body, &[("关闭", "ok", "suggested-action")], on_resp);
}

/// 信息框（只读内容）
pub fn info(parent: &gtk4::Window, title: &str, body: &str) {
    let on_resp: Rc<dyn Fn(&str)> = Rc::new(|_| {});
    choice_dialog(parent, title, body, &[("关闭", "ok", "")], on_resp);
}

/// 下载失败框（v3.6.2）。
///
/// 起因：用户实测反复遇到 `[Api] EOF`——下载中断后 `blobs/` 里留下未完成的分片，
/// 下次拉取撞上这些残片时 Ollama 直接断开，流内只回 `error="EOF"`。
/// 该错误原文对用户零信息量，且**每一次失败位置都不同**（取决于先撞上哪个残片），
/// 这让用户误以为「下载随机失败、完全没规律」。
///
/// 本对话框给出三件事：为什么失败、为什么每次位置不同、以及一键修复入口。
/// 点「清理残留并重试」→ 删除 `-partial*` 残片 → 重新发起同一次拉取。
pub fn pull_failed(parent: &gtk4::Window, model: &str, detail: &str, on_retry: impl Fn() + 'static) {
    let on_retry = Rc::new(on_retry);
    let on_resp = Rc::new(move |id: &str| {
        if id == "retry" {
            on_retry();
        }
    });
    let body = format!(
        "{detail}\n\n\
         ── 关于「每次失败的位置都不一样」 ──\n\
         下载模型是一边写盘一边校验的过程，中途断开会在模型目录留下未完成的分片文件。\n\
         下次拉取时，Ollama 会尝试续用这些残片；一旦残片与实际进度对不上，\n\
         它就直接中止本次下载，并把错误简化成一句 `EOF`。\n\
         所以「撞上哪一片、失败在第几阶段」每次都不同，这不是随机故障，\n\
         而是残留文件的状态决定的结果。\n\n\
         点「清理残留并重试」会自动删除这些未完成的分片（**已装好的模型完全不受影响**），\n\
         然后重新开始下载。"
    );
    choice_dialog(
        parent,
        &format!("下载失败：{model}"),
        &body,
        &[
            ("取消", "cancel", ""),
            ("清理残留并重试", "retry", "suggested-action"),
        ],
        on_resp,
    );
}

/// 详情窗口：内容较长（如 Modelfile / 参数 / 许可证）时可滚动、可选中、可复制。
/// 比信息框更合适：信息框会把长文本挤在固定高度里且无法选中复制。
pub fn details(parent: &gtk4::Window, title: &str, body: &str) {
    let win = gtk4::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(title)
        .default_width(760)
        .default_height(560)
        .build();

    let buf = gtk4::TextBuffer::builder().build();
    buf.set_text(body);
    let view = gtk4::TextView::builder()
        .buffer(&buf)
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .build();
    let scroll = gtk4::ScrolledWindow::builder()
        .child(&view)
        .vexpand(true)
        .build();

    let body_owned = body.to_string();
    let copy_btn = gtk4::Button::builder().label("复制").build();
    copy_btn.connect_clicked(move |_| {
        if let Some(display) = gtk4::gdk::Display::default() {
            display.clipboard().set_text(&body_owned);
        }
    });
    let close_btn = gtk4::Button::builder()
        .label("关闭")
        .css_classes(vec!["suggested-action"])
        .build();
    let w = win.clone();
    close_btn.connect_clicked(move |_| w.close());

    let bar = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk4::Align::End)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();
    bar.append(&copy_btn);
    bar.append(&close_btn);

    let vbox = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .build();
    vbox.append(&scroll);
    vbox.append(&bar);
    win.set_child(Some(&vbox));
    win.present();
}

type InputGetter = Rc<dyn Fn() -> String>;

fn make_getter(entry: gtk4::Entry) -> InputGetter {
    Rc::new(move || entry.text().to_string())
}

fn make_area_getter(buf: gtk4::TextBuffer) -> InputGetter {
    Rc::new(move || {
        let start = buf.start_iter();
        let end = buf.end_iter();
        buf.text(&start, &end, false).to_string()
    })
}

/// 单行文本输入框
pub fn text_input(
    parent: &gtk4::Window,
    title: &str,
    label: &str,
    default: &str,
    confirm_label: &str,
    on_submit: impl Fn(Option<String>) + 'static,
) {
    let win = gtk4::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(title)
        .default_width(440)
        .build();

    let lbl = gtk4::Label::builder()
        .label(label)
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    let entry = gtk4::Entry::builder()
        .text(default)
        .activates_default(true)
        .hexpand(true)
        .width_request(340)
        .build();
    let row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(8)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    row.append(&lbl);
    row.append(&entry);

    let getter = make_getter(entry.clone());
    let on_submit2 = Rc::new(on_submit);
    let ok_cb = on_submit2.clone();
    let cancel_cb = on_submit2.clone();
    let enter_cb = on_submit2.clone();
    let getter_ok = getter.clone();
    let getter_enter = getter.clone();
    let win_ok = win.clone();
    let win_cancel = win.clone();
    let win_enter = win.clone();

    let ok = gtk4::Button::builder()
        .label(confirm_label)
        .css_classes(vec!["suggested-action"])
        .build();
    let cancel = gtk4::Button::builder().label("取消").build();
    ok.connect_clicked(move |_| {
        ok_cb(Some(getter_ok()));
        win_ok.close();
    });
    cancel.connect_clicked(move |_| {
        cancel_cb(None);
        win_cancel.close();
    });
    // 回车确认
    entry.connect_activate(move |_| {
        enter_cb(Some(getter_enter()));
        win_enter.close();
    });
    // 底部按钮
    let bar = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk4::Align::End)
        .margin_bottom(12)
        .margin_end(16)
        .build();
    bar.append(&cancel);
    bar.append(&ok);
    row.append(&bar);

    win.set_child(Some(&row));
    win.present();
}

/// 多行文本输入框（Modelfile 编辑）
pub fn text_area(
    parent: &gtk4::Window,
    title: &str,
    label: &str,
    default: &str,
    confirm_label: &str,
    on_submit: impl Fn(Option<String>) + 'static,
) {
    let win = gtk4::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(title)
        .default_width(540)
        .build();

    let lbl = gtk4::Label::builder()
        .label(label)
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    let buf = gtk4::TextBuffer::builder().build();
    buf.set_text(default);
    let view = gtk4::TextView::builder()
        .buffer(&buf)
        .height_request(240)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .css_classes(vec!["monospace"])
        .build();
    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&view)
        .build();

    let vbox = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(8)
        .margin_top(16)
        .margin_bottom(8)
        .margin_start(16)
        .margin_end(16)
        .build();
    vbox.append(&lbl);
    vbox.append(&scroll);
    win.set_child(Some(&vbox));

    let getter = make_area_getter(buf);
    let on_submit2 = Rc::new(on_submit);
    let ok_cb = on_submit2.clone();
    let cancel_cb = on_submit2.clone();
    let win_ok = win.clone();
    let win_cancel = win.clone();
    let ok = gtk4::Button::builder()
        .label(confirm_label)
        .css_classes(vec!["suggested-action"])
        .build();
    let cancel = gtk4::Button::builder().label("取消").build();
    ok.connect_clicked(move |_| {
        ok_cb(Some(getter()));
        win_ok.close();
    });
    cancel.connect_clicked(move |_| {
        cancel_cb(None);
        win_cancel.close();
    });
    let bar = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk4::Align::End)
        .margin_bottom(12)
        .margin_end(16)
        .build();
    bar.append(&cancel);
    bar.append(&ok);
    vbox.append(&bar);

    win.present();
}

/// 进度对话框
///
/// v3.5.0：长任务（拉取/推送）改为**非模态 + 可取消**。
/// 原实现是 modal 且没有任何按钮——下载一个 20GB 模型期间整个应用被挡住
/// 且无法中断，违反「用户控制与自由」（Nielsen #3）。
pub struct ProgressDialog {
    pub dialog: gtk4::Window,
    pub label: gtk4::Label,
    pub bar: gtk4::ProgressBar,
    /// 用户点「取消」或关闭窗口即置位，长任务循环据此中断
    cancel: Arc<AtomicBool>,
    /// 不确定进度动画的定时器源；切到确定进度时移除它
    pulse: std::cell::RefCell<Option<glib::SourceId>>,
}

impl ProgressDialog {
    pub fn set_text(&self, t: &str) {
        self.label.set_label(t);
    }
    pub fn set_fraction(&self, f: f64) {
        // 切到确定进度：停掉脉冲动画，避免覆盖进度填充
        if let Some(src) = self.pulse.borrow_mut().take() {
            src.remove();
        }
        self.bar.set_fraction(f.clamp(0.0, 1.0));
    }
    /// 该进度框的取消标志（交给长任务循环轮询）
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }
    /// 用户是否已请求取消
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    pub fn close(&self) {
        if let Some(src) = self.pulse.borrow_mut().take() {
            src.remove();
        }
        self.dialog.close();
    }
}

/// 创建进度对话框。
///
/// `cancelable = true`：非模态 + 显示「取消」按钮（关闭窗口等同于取消）。
/// 用于耗时不可预期的长任务（拉取/推送模型）。
/// `cancelable = false`：保持模态、无按钮，用于秒级短任务（创建模型）。
pub fn progress(parent: &gtk4::Window, title: &str, cancelable: bool) -> Rc<ProgressDialog> {
    let label = gtk4::Label::builder()
        .label("准备中…")
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    let bar = gtk4::ProgressBar::builder().fraction(0.0).pulse_step(0.05).build();
    let vbox = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    vbox.append(&label);
    vbox.append(&bar);

    let cancel: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

    if cancelable {
        let hint = gtk4::Label::builder()
            .label("可继续使用应用；点「取消」或关闭本窗口即中断下载。")
            .halign(gtk4::Align::Start)
            .wrap(true)
            .css_classes(vec!["caption", "dim-label"])
            .build();
        vbox.append(&hint);

        let cancel_btn = gtk4::Button::builder()
            .label("取消")
            .halign(gtk4::Align::End)
            .css_classes(vec!["destructive-action"])
            .build();
        let row = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .halign(gtk4::Align::End)
            .build();
        row.append(&cancel_btn);
        vbox.append(&row);

        let c_flag = Arc::clone(&cancel);
        let lbl = label.clone();
        let btn = cancel_btn.clone();
        cancel_btn.connect_clicked(move |_| {
            c_flag.store(true, Ordering::Relaxed);
            btn.set_sensitive(false);
            btn.set_label("正在取消…");
            lbl.set_label("正在中断下载…");
        });
    }

    let win = gtk4::Window::builder()
        .title(title)
        .modal(!cancelable)
        .transient_for(parent)
        .default_width(440)
        .build();
    win.set_child(Some(&vbox));

    // 关闭窗口 = 取消（长任务时避免「窗口没了但下载还在跑」的歧义）
    if cancelable {
        let c_flag = Arc::clone(&cancel);
        win.connect_close_request(move |_| {
            c_flag.store(true, Ordering::Relaxed);
            glib::Propagation::Proceed
        });
    }

    win.present();

    // 不确定进度动画（v1.0.15：80ms→300ms——拉取期间 12.5Hz 的重绘会加重
    // 主循环与显示通道压力，是下载卡死线索的高危项，视觉观感差异可忽略）
    let bar2 = bar.clone();
    let pulse = glib::timeout_add_local(std::time::Duration::from_millis(300), move || {
        bar2.pulse();
        glib::ControlFlow::Continue
    });

    Rc::new(ProgressDialog {
        dialog: win,
        label,
        bar,
        cancel,
        pulse: std::cell::RefCell::new(Some(pulse)),
    })
}

/// 错误信息框
pub fn show_err(parent: &gtk4::Window, e: &str) {
    info(parent, "操作失败", e);
}

/// 发送桌面通知（GTK 通知，非模态、不打断操作）。失败静默忽略。
pub fn notify(win: &gtk4::Window, title: &str, body: &str) {
    if let Some(app) = win.application() {
        let n = gtk4::gio::Notification::new(title);
        n.set_body(Some(body));
        app.send_notification(None, &n);
    }
}

/// 服务日志窗：可滚动查看 Ollama 运行日志，附「刷新」按钮重新读取。
/// 日志来源由 `service::read_logs` 决定（journalctl 优先，其次日志文件），
/// 二者皆空时给出解释性提示，不伪造空日志。
pub fn server_log(parent: &gtk4::Window) {
    use crate::rt;
    let win = gtk4::Window::builder()
        .transient_for(parent)
        .title("Ollama 服务日志")
        .default_width(820)
        .default_height(560)
        .build();

    let buf = gtk4::TextBuffer::builder().build();
    let view = gtk4::TextView::builder()
        .buffer(&buf)
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .build();
    let scroll = gtk4::ScrolledWindow::builder().child(&view).vexpand(true).build();

    let refresh_btn = gtk4::Button::builder().label("刷新").css_classes(vec!["suggested-action"]).build();
    let close_btn = gtk4::Button::builder().label("关闭").build();
    let w = win.clone();
    close_btn.connect_clicked(move |_| w.close());

    let bar = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk4::Align::End)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();
    bar.append(&refresh_btn);
    bar.append(&close_btn);

    let vbox = gtk4::Box::builder().orientation(gtk4::Orientation::Vertical).build();
    vbox.append(&scroll);
    vbox.append(&bar);
    win.set_child(Some(&vbox));
    win.present();

    let buf_c = buf.clone();
    let load = move || {
        let buf = buf_c.clone();
        rt::spawn(
            async move { crate::service::read_logs() },
            move |text: String| {
                buf.set_text(&text);
            },
        );
    };
    let load_cb = load.clone();
    refresh_btn.connect_clicked(move |_| load_cb());
    load();
}

fn opt_to_string<T: std::fmt::Display>(v: Option<T>) -> String {
    match v {
        Some(x) => x.to_string(),
        None => String::new(),
    }
}

fn parse_num<T: std::str::FromStr>(s: &str) -> Option<T> {
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        s.parse::<T>().ok()
    }
}

/// 停止词：逗号分隔解析为字符串数组
fn parse_stop(s: &str) -> Option<Vec<String>> {
    let v: Vec<String> = s
        .split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// keep_alive：空串 → None（用服务端默认）；否则保留原始字符串（如 5m / -1 / 0）
fn parse_keepalive(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// 生成参数编辑对话框：编辑 temperature / top_p / top_k / num_ctx /
/// num_predict / seed / repeat_penalty 与 system 提示，确定后回传完整 GenParams。
pub fn gen_params(parent: &gtk4::Window, current: &crate::ollama::GenParams, on_submit: impl Fn(crate::ollama::GenParams) + 'static) {
    use crate::ollama::GenParams;
    let content = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(16)
        .margin_end(16)
        .build();

    // ---- System 系统提示（多行）----
    let sys_label = gtk4::Label::builder()
        .label("System 系统提示")
        .halign(gtk4::Align::Start)
        .css_classes(vec!["title-4", "heading"])
        .build();
    let sys_buf = gtk4::TextBuffer::builder().build();
    sys_buf.set_text(&current.system.clone().unwrap_or_default());
    let sys_view = gtk4::TextView::builder()
        .buffer(&sys_buf)
        .height_request(72)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .css_classes(vec!["monospace"])
        .build();
    let sys_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .min_content_height(72)
        .child(&sys_view)
        .build();
    content.append(&sys_label);
    content.append(&sys_scroll);

    // ---- 数值字段 ----
    let content_c = content.clone();
    let mk_field = move |label: &str, val: &str| -> gtk4::Entry {
        let l = gtk4::Label::builder()
            .label(label)
            .halign(gtk4::Align::Start)
            .width_request(150)
            .build();
        let e = gtk4::Entry::builder().text(val).hexpand(true).build();
        let row = gtk4::Box::builder().orientation(gtk4::Orientation::Horizontal).spacing(8).build();
        row.append(&l);
        row.append(&e);
        content_c.append(&row);
        e
    };

    let f_temp = mk_field("温度 temperature", &opt_to_string(current.temperature));
    let f_topp = mk_field("top_p", &opt_to_string(current.top_p));
    let f_topk = mk_field("top_k", &opt_to_string(current.top_k));
    let f_ctx = mk_field("上下文 num_ctx", &opt_to_string(current.num_ctx));
    let f_pred = mk_field("最大生成 num_predict", &opt_to_string(current.num_predict));
    let f_seed = mk_field("seed", &opt_to_string(current.seed));
    let f_rep = mk_field("重复惩罚 repeat_penalty", &opt_to_string(current.repeat_penalty));
    let f_ka = mk_field("keep_alive (如 5m / -1常驻 / 0卸)", &current.keep_alive.clone().unwrap_or_default());
    let f_stop = mk_field("停止词 stop (逗号分隔)", &current.stop.clone().map(|v| v.join(", ")).unwrap_or_default());
    let f_minp = mk_field("min_p", &opt_to_string(current.min_p));
    let f_typ = mk_field("typical_p", &opt_to_string(current.typical_p));
    let f_pres = mk_field("presence_penalty", &opt_to_string(current.presence_penalty));
    let f_freq = mk_field("frequency_penalty", &opt_to_string(current.frequency_penalty));
    let f_gpu = mk_field("num_gpu", &opt_to_string(current.num_gpu));

    // ---- 工具定义 tools（函数调用）：粘贴 Ollama 格式的 JSON 数组，留空则不启用 ----
    let tools_label = gtk4::Label::builder()
        .label("工具定义 tools（函数调用 · JSON 数组，留空则不启用）")
        .halign(gtk4::Align::Start)
        .css_classes(vec!["title-4", "heading"])
        .margin_top(8)
        .build();
    let tools_init = current
        .tools
        .as_ref()
        .map(|v| serde_json::to_string_pretty(v).unwrap_or_default())
        .unwrap_or_default();
    let tools_buf = gtk4::TextBuffer::builder().build();
    tools_buf.set_text(&tools_init);
    let tools_view = gtk4::TextView::builder()
        .buffer(&tools_buf)
        .height_request(96)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .css_classes(vec!["monospace"])
        .build();
    let tools_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .min_content_height(96)
        .child(&tools_view)
        .build();
    content.append(&tools_label);
    content.append(&tools_scroll);

    // ---- 参数预设：一键填充常用采样档位（确定后再点「应用」生效）----
    let preset_row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(6)
        .margin_top(6)
        .build();
    preset_row.append(&gtk4::Label::builder().label("预设：").css_classes(vec!["dim-label"]).build());
    let apply_preset = |name: &str, temp: f32, top_p: f32, top_k: u32, num_pred: u32, rep: f32| {
        let tip: &str = match name {
            "创意" => "更高温度与多样性，适合发散",
            "精确" => "更低温度，输出更稳定确定",
            _ => "更少生成 token，响应更快",
        };
        let b = gtk4::Button::builder()
            .label(name)
            .css_classes(vec!["flat"])
            .tooltip_text(tip)
            .build();
        let (ft, fp, fk, fpr, fr) = (
            f_temp.clone(),
            f_topp.clone(),
            f_topk.clone(),
            f_pred.clone(),
            f_rep.clone(),
        );
        b.connect_clicked(move |_| {
            ft.set_text(&temp.to_string());
            fp.set_text(&top_p.to_string());
            fk.set_text(&top_k.to_string());
            fpr.set_text(&num_pred.to_string());
            fr.set_text(&rep.to_string());
        });
        preset_row.append(&b);
    };
    apply_preset("创意", 1.1, 0.95, 40, 1024, 1.1);
    apply_preset("精确", 0.3, 0.9, 20, 512, 1.05);
    apply_preset("快速", 0.7, 0.9, 40, 128, 1.0);
    content.append(&preset_row);

    // ---- 底部按钮 ----
    let btn_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk4::Align::End)
        .margin_top(10)
        .build();
    let cancel_btn = gtk4::Button::builder().label("取消").build();
    let ok_btn = gtk4::Button::builder().label("应用").css_classes(vec!["suggested-action"]).build();
    btn_box.append(&cancel_btn);
    btn_box.append(&ok_btn);
    content.append(&btn_box);

    let win_dialog = gtk4::Window::builder()
        .transient_for(parent)
        .title("生成参数")
        .default_width(460)
        .build();
    win_dialog.set_child(Some(&content));
    win_dialog.present();

    let win_c = win_dialog.clone();
    cancel_btn.connect_clicked(move |_| win_c.close());

    // 先把需要从 current 取的字段转成拥有值，避免把短生命周期引用捕获进 'static 闭包
    let cur_think = current.think;
    let cur_format = current.format.clone();

    let win_o = win_dialog.clone();
    ok_btn.connect_clicked(move |_| {
        let sys = {
            let start = sys_buf.start_iter();
            let end = sys_buf.end_iter();
            let t = sys_buf.text(&start, &end, false).to_string();
            let t = t.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        };
        let tools_text = {
            let start = tools_buf.start_iter();
            let end = tools_buf.end_iter();
            tools_buf.text(&start, &end, false).to_string()
        };
        let tools = if tools_text.trim().is_empty() {
            None
        } else {
            match serde_json::from_str::<serde_json::Value>(&tools_text) {
                Ok(serde_json::Value::Array(arr)) => Some(arr),
                Ok(_) => {
                    info(
                        &win_dialog,
                        "工具定义无效",
                        "tools 必须是一个 JSON 数组，形如：\n[{\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"description\":\"...\",\"parameters\":{...}}}]",
                    );
                    return;
                }
                Err(e) => {
                    info(&win_dialog, "工具定义无效", &format!("JSON 解析失败：{e}"));
                    return;
                }
            }
        };
        let params = GenParams {
            system: sys,
            temperature: parse_num::<f32>(&f_temp.text()),
            top_p: parse_num::<f32>(&f_topp.text()),
            top_k: parse_num::<u32>(&f_topk.text()),
            num_ctx: parse_num::<u32>(&f_ctx.text()),
            num_predict: parse_num::<u32>(&f_pred.text()),
            seed: parse_num::<u32>(&f_seed.text()),
            repeat_penalty: parse_num::<f32>(&f_rep.text()),
            stop: parse_stop(&f_stop.text()),
            min_p: parse_num::<f32>(&f_minp.text()),
            typical_p: parse_num::<f32>(&f_typ.text()),
            presence_penalty: parse_num::<f32>(&f_pres.text()),
            frequency_penalty: parse_num::<f32>(&f_freq.text()),
            penalize_newline: None,
            numa: None,
            num_gpu: parse_num::<i32>(&f_gpu.text()),
            main_gpu: None,
            num_batch: None,
            draft_num_predict: None,
            keep_alive: parse_keepalive(&f_ka.text()),
            think: cur_think,
            format: cur_format.clone(),
            tools,
        };
        on_submit(params);
        win_o.close();
    });
}

/// 注册中心登录：输入 registry 地址与 token，确定后回传 (registry, token)
pub fn registry_login(parent: &gtk4::Window, on_submit: impl Fn((String, String)) + 'static) {
    let win = gtk4::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("注册中心登录")
        .default_width(460)
        .build();
    let vbox = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(10)
        .margin_top(16)
        .margin_bottom(12)
        .margin_start(16)
        .margin_end(16)
        .build();

    let reg_lbl = gtk4::Label::builder()
        .label("Registry 地址")
        .halign(gtk4::Align::Start)
        .build();
    let reg = gtk4::Entry::builder()
        .text("https://ollama.com")
        .hexpand(true)
        .build();
    let tok_lbl = gtk4::Label::builder()
        .label("Token（ollama.com 用账户令牌；私有仓库用仓库令牌）")
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    let tok = gtk4::PasswordEntry::builder()
        .hexpand(true)
        .show_peek_icon(true)
        .build();

    vbox.append(&reg_lbl);
    vbox.append(&reg);
    vbox.append(&tok_lbl);
    vbox.append(&tok);

    let ok = gtk4::Button::builder()
        .label("登录")
        .css_classes(vec!["suggested-action"])
        .build();
    let cancel = gtk4::Button::builder().label("取消").build();
    let bar = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk4::Align::End)
        .build();
    bar.append(&cancel);
    bar.append(&ok);
    vbox.append(&bar);
    win.set_child(Some(&vbox));

    let win_c = win.clone();
    let ok_cb = Rc::new(on_submit);
    let win_cancel = win.clone();
    cancel.connect_clicked(move |_| win_cancel.close());
    ok.connect_clicked(move |_| {
        let r = reg.text().to_string();
        let t = tok.text().to_string();
        if !r.trim().is_empty() && !t.trim().is_empty() {
            ok_cb((r.trim().to_string(), t.trim().to_string()));
            win_c.close();
        }
    });
    win.present();
}

/// 注册中心登出：列出已登录 registry，选择其一删除
pub fn registry_logout(parent: &gtk4::Window, on_submit: impl Fn(String) + 'static) {
    let regs = crate::registry::registries();
    if regs.is_empty() {
        info(parent, "未登录", "当前没有任何已保存的注册中心鉴权。");
        return;
    }
    let win = gtk4::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("注册中心登出")
        .default_width(420)
        .build();
    let vbox = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(10)
        .margin_top(16)
        .margin_bottom(12)
        .margin_start(16)
        .margin_end(16)
        .build();
    let lbl = gtk4::Label::builder()
        .label("选择要登出的注册中心")
        .halign(gtk4::Align::Start)
        .build();
    let regs_refs: Vec<&str> = regs.iter().map(|s| s.as_str()).collect();
    let dd = gtk4::DropDown::from_strings(&regs_refs);
    vbox.append(&lbl);
    vbox.append(&dd);

    let ok = gtk4::Button::builder()
        .label("登出")
        .css_classes(vec!["destructive-action"])
        .build();
    let cancel = gtk4::Button::builder().label("取消").build();
    let bar = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk4::Align::End)
        .build();
    bar.append(&cancel);
    bar.append(&ok);
    vbox.append(&bar);
    win.set_child(Some(&vbox));

    let win_c = win.clone();
    let ok_cb = Rc::new(on_submit);
    let win_cancel = win.clone();
    cancel.connect_clicked(move |_| win_cancel.close());
    ok.connect_clicked(move |_| {
        let i = dd.selected() as usize;
        if i < regs.len() {
            ok_cb(regs[i].clone());
            win_c.close();
        }
    });
    win.present();
}

/// 模型库浏览器：输入模型名 → 查询 Ollama 官方库标签 → 点选标签即按 `模型:标签` 拉取。
/// 联网查询 registry.ollama.ai；离线或仓库不可达时如实提示，不伪造结果。
pub fn library_browser(parent: &gtk4::Window, on_pull: impl Fn(String) + 'static) {
    let win = gtk4::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("浏览模型库")
        .default_width(480)
        .default_height(520)
        .build();

    let vbox = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let tip = gtk4::Label::builder()
        .label("输入模型名查询官方库标签（如 llama3.2、qwen2.5、gemma2），点选标签即拉取。")
        .css_classes(vec!["dim-label"])
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    vbox.append(&tip);

    let row = gtk4::Box::builder().orientation(gtk4::Orientation::Horizontal).spacing(8).build();
    let entry = gtk4::Entry::builder()
        .hexpand(true)
        .placeholder_text("模型名，如 llama3.2")
        .activates_default(true)
        .build();
    let query_btn = gtk4::Button::builder().label("查询").css_classes(vec!["suggested-action"]).build();
    row.append(&entry);
    row.append(&query_btn);
    vbox.append(&row);

    let status = gtk4::Label::builder()
        .label("")
        .css_classes(vec!["dim-label"])
        .halign(gtk4::Align::Start)
        .build();
    vbox.append(&status);

    let list = gtk4::ListBox::builder().vexpand(true).build();
    list.set_activate_on_single_click(true);
    // 每行对应的「模型:标签」全名，点选时按索引取出（ListBoxRow 无 connect_activated）
    let tag_fulls: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let scroll = gtk4::ScrolledWindow::builder().child(&list).vexpand(true).build();
    vbox.append(&scroll);

    let on_pull = Rc::new(on_pull);
    // 点选列表项即按「模型:标签」拉取（注意：它取的是当前查询结果的对应索引）
    {
        let list = list.clone();
        let tag_fulls = tag_fulls.clone();
        let on_pull = on_pull.clone();
        let win = win.clone();
        list.connect_row_activated(move |_, row| {
            let idx = row.index();
            if idx < 0 {
                return;
            }
            let full = tag_fulls.borrow().get(idx as usize).cloned();
            if let Some(full) = full {
                on_pull(full);
                win.close();
            }
        });
    }
    let do_query: Rc<dyn Fn()> = Rc::new({
        let win = win.clone();
        let entry = entry.clone();
        let status = status.clone();
        let list = list.clone();
        let on_pull = on_pull.clone();
        move || {
            let model = entry.text().to_string().trim().to_string();
            if model.is_empty() {
                return;
            }
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            status.set_label("查询中…");
            let client = crate::ollama::client();
            let model_c = model.clone();
            let model_for_cb = model_c.clone();
            let status_c = status.clone();
            let list_c = list.clone();
            let win_c = win.clone();
            let on_pull = on_pull.clone();
            let tag_fulls = tag_fulls.clone();
            tag_fulls.borrow_mut().clear();
            crate::rt::spawn(
                async move { crate::ollama::library::fetch_tags(&client, &model_c).await.map_err(|e| e.to_string()) },
                move |res: Result<Vec<crate::ollama::library::LibraryTag>, String>| {
                    match res {
                        Ok(tags) => {
                            if tags.is_empty() {
                                status_c.set_label("未找到该模型的标签（请检查模型名拼写）");
                                return;
                            }
                            status_c.set_label(&format!("共 {} 个标签，点选即拉取", tags.len()));
                            for tag in tags {
                                let full = format!("{}:{}", model_for_cb, tag.tag);
                                let r = gtk4::ListBoxRow::new();
                                let b = gtk4::Box::builder()
                                    .orientation(gtk4::Orientation::Vertical)
                                    .spacing(1)
                                    .margin_start(8)
                                    .margin_end(8)
                                    .margin_top(4)
                                    .margin_bottom(4)
                                    .build();
                                b.append(
                                    &gtk4::Label::builder()
                                        .label(&full)
                                        .halign(gtk4::Align::Start)
                                        .build(),
                                );
                                b.append(
                                    &gtk4::Label::builder()
                                        .label(&format!("{}", tag.size_human))
                                        .css_classes(vec!["caption", "dim-label"])
                                        .halign(gtk4::Align::Start)
                                        .build(),
                                );
                                r.set_child(Some(&b));
                                list_c.append(&r);
                                tag_fulls.borrow_mut().push(full);
                            }
                        }
                        Err(e) => status_c.set_label(&format!("查询失败：{e}")),
                    }
                },
            );
        }
    });
    query_btn.connect_clicked({ let d = do_query.clone(); move |_| d() });
    entry.connect_activate({ let d = do_query.clone(); move |_| d() });

    win.set_child(Some(&vbox));
    win.present();
}
