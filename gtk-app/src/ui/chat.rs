//! 右侧聊天区（GTK4 原生）：多轮上下文 + 流式对话 + 输入

use crate::ollama::{self, ChatMessage, GenParams};
use crate::rt;
use crate::rt::StreamMsg;
use gtk4::gdk;
use gtk4::prelude::*;
use gtk4::{Box, ComboBoxText, Label, Orientation, ScrolledWindow, TextBuffer, TextView};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 单个会话的持久化数据（多会话：换模型不再丢历史）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Session {
    pub id: String,
    pub title: String,
    /// 该会话绑定的模型（每条会话可不同；None 表示尚未选模型）
    pub model: Option<String>,
    /// 完整多轮对话上下文
    pub messages: Vec<ChatMessage>,
}

/// 会话持久化文件结构
#[derive(Debug, Serialize, Deserialize, Default)]
struct SessionsFile {
    sessions: Vec<Session>,
    current: usize,
}

/// 聊天区状态
pub struct ChatPane {
    /// 对话记录缓冲区（只读展示，含历史消息与角色前缀）
    pub history: TextBuffer,
    /// 输入区缓冲区（可编辑）
    pub input: TextBuffer,
    /// 当前选中模型（当前活动会话）
    pub model: RefCell<Option<String>>,
    /// 正在生成
    pub streaming: RefCell<bool>,
    /// 多轮对话上下文（完整发给 Ollama 的 messages，当前活动会话）
    pub messages: RefCell<Vec<ChatMessage>>,
    /// 生成参数（可在界面调整）
    pub params: RefCell<GenParams>,
    /// 本次对话附带的图片（base64，多模态）。清空对话时一并清空。
    pub images: RefCell<Vec<String>>,
    /// 当前生成的取消标志（点「停止」时置 true，后台读取后中断）
    pub cancel: RefCell<Option<Arc<AtomicBool>>>,
    /// 全部会话（持久化到磁盘）
    pub sessions: RefCell<Vec<Session>>,
    /// 当前活动会话索引
    pub current: RefCell<usize>,
    /// 会话侧栏列表（v3.1 重设计：ListBox 替代下拉框）
    pub session_list: gtk4::ListBox,
    /// 对话页内直接选择本地已装模型（v1.0.17）
    pub model_combo: ComboBoxText,
    /// 程序化改下拉框时抑制 changed 回调（避免递归切换）
    suppress_combo: RefCell<bool>,
    pub title: Label,
    pub container: Box,
    /// 流式生成状态条（生成中显示，结束隐藏）
    pub status_row: gtk4::Box,
    pub input_view: TextView,
    pub win: gtk4::Window,
    // 控件引用：按生成状态切换可用性
    stop_btn: gtk4::Button,
    send_btn: gtk4::Button,
    regen_btn: gtk4::Button,
}

/// 在缓冲区末尾追加纯文本（无 markup，免转义风险）
fn buf_append(buf: &TextBuffer, s: &str) {
    let mut it = buf.end_iter();
    buf.insert(&mut it, s);
}

/// 标准 Base64 编码（多模态图片送 Ollama 时需要将图片转 base64，避免引入额外依赖）
fn to_base64(data: &[u8]) -> String {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { B64[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// 构建聊天区，返回 (容器, 可操作句柄)
pub fn build_chat_pane(parent: &gtk4::Window) -> (gtk4::Widget, Rc<RefCell<ChatPane>>) {
    let title = gtk4::Label::builder()
        .label("选择左侧模型开始对话")
        .css_classes(vec!["title-3", "heading"])
        .halign(gtk4::Align::Start)
        .build();

    // 对话记录区（只读）
    let history = TextBuffer::builder().build();
    // 注册 Markdown 渲染所需的标签（流式收尾时把助手消息重渲染为富文本）
    crate::ui::markdown::ensure_md_tags(&history);
    let history_view = TextView::builder()
        .buffer(&history)
        .editable(false)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .build();
    let history_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&history_view)
        .build();

    // 输入区：外面包一层 Frame 边框，即使无文字也有清晰可见的输入框轮廓
    let input = TextBuffer::builder().build();
    let input_view = TextView::builder()
        .buffer(&input)
        .height_request(84)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .css_classes(vec!["monospace"])
        .build();
    let input_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .min_content_height(84)
        .child(&input_view)
        .build();

    // 占位提示：输入为空时显示灰色引导文字
    let placeholder = gtk4::Label::builder()
        .label("输入消息，Enter 发送 · Shift+Enter 换行")
        .css_classes(vec!["dim-label"])
        .halign(gtk4::Align::Start)
        .margin_start(8)
        .margin_top(6)
        .build();
    let input_overlay = gtk4::Overlay::new();
    input_overlay.set_child(Some(&input_scroll));
    input_overlay.add_overlay(&placeholder);
    input.connect_changed(glib::clone!(@weak placeholder, @weak input => move |_| {
        let empty = {
            let start = input.start_iter();
            let end = input.end_iter();
            input.text(&start, &end, false).is_empty()
        };
        placeholder.set_visible(empty);
    }));

    let input_frame = gtk4::Frame::builder()
        .css_classes(vec!["view"])
        .child(&input_overlay)
        .build();

    // ---- 操作行 ----
    // 左侧：会话管理（清空 / 导出 / 重新生成）；右侧：参数 / 停止 / 发送
    let send_btn = gtk4::Button::builder()
        .label("发送")
        .css_classes(vec!["suggested-action"])
        .build();
    let stop_btn = gtk4::Button::builder()
        .label("停止")
        .icon_name("media-playback-stop-symbolic")
        .tooltip_text("中断本次生成")
        .css_classes(vec!["destructive-action"])
        .build();
    stop_btn.set_sensitive(false); // 仅生成过程中可用
    let params_btn = gtk4::Button::builder()
        .label("参数")
        .icon_name("preferences-other-symbolic")
        .tooltip_text("调整生成参数（system / temperature 等）")
        .css_classes(vec!["flat"])
        .build();
    let clear_btn = gtk4::Button::builder()
        .icon_name("edit-clear-symbolic")
        .tooltip_text("清空当前对话")
        .css_classes(vec!["flat"])
        .build();
    let export_btn = gtk4::Button::builder()
        .icon_name("document-save-symbolic")
        .tooltip_text("导出对话为 Markdown 文件")
        .css_classes(vec!["flat"])
        .build();
    let regen_btn = gtk4::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("重新生成上一条回复")
        .css_classes(vec!["flat"])
        .build();

    let send_row = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .build();
    send_row.append(&clear_btn);
    send_row.append(&export_btn);
    send_row.append(&regen_btn);
    let row_spacer = Box::builder().orientation(Orientation::Horizontal).hexpand(true).build();
    send_row.append(&row_spacer);
    send_row.append(&stop_btn);
    send_row.append(&send_btn);

    // ---- 选项行：图片（多模态）/ 思考过程 / 输出格式 ----
    let img_btn = gtk4::Button::builder()
        .icon_name("image-x-generic-symbolic")
        .tooltip_text("附加图片（多模态，随本条消息发送）")
        .css_classes(vec!["flat"])
        .build();
    let img_lbl = gtk4::Label::builder().label("").css_classes(vec!["dim-label"]).build();
    let think_sw = gtk4::Switch::builder().valign(gtk4::Align::Center).build();
    let think_lbl = gtk4::Label::builder().label("思考").css_classes(vec!["dim-label"]).build();
    let fmt_dd = gtk4::DropDown::from_strings(&["纯文本", "JSON", "自定义 Schema"]);
    let opt_row = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .margin_top(2)
        .margin_bottom(4)
        .build();
    opt_row.append(&img_btn);
    opt_row.append(&img_lbl);
    let think_box = Box::builder().orientation(Orientation::Horizontal).spacing(4).build();
    think_box.append(&think_lbl);
    think_box.append(&think_sw);
    opt_row.append(&think_box);
    opt_row.append(&gtk4::Label::builder().label("格式").css_classes(vec!["dim-label"]).build());
    opt_row.append(&fmt_dd);

    // 自定义 JSON Schema 输入区（仅当选「自定义 Schema」时显示）
    let schema_buf = gtk4::TextBuffer::builder().build();
    let schema_view = gtk4::TextView::builder()
        .buffer(&schema_buf)
        .height_request(72)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .css_classes(vec!["monospace"])
        .build();
    let schema_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .min_content_height(72)
        .child(&schema_view)
        .tooltip_text("粘贴 JSON Schema（对象），模型将按该结构输出。留空则不使用格式约束")
        .build();
    schema_scroll.set_visible(false);

    let container = Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(10)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    // ---- 会话侧栏（v3.1 重设计）：标题 + 新建按钮 + ListBox 会话列表 ----
    let session_list = gtk4::ListBox::builder()
        .css_classes(vec!["navigation-sidebar"])
        .tooltip_text("点击切换会话（换模型不再丢历史）")
        .build();
    let new_session_btn = gtk4::Button::builder()
        .label("新建")
        .icon_name("list-add-symbolic")
        .tooltip_text("新建会话")
        .css_classes(vec!["suggested-action"])
        .build();
    let side_lbl = gtk4::Label::builder()
        .label("对话")
        .css_classes(vec!["title-4", "heading"])
        .halign(gtk4::Align::Start)
        .hexpand(true)
        .build();
    let side_head = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .build();
    side_head.append(&side_lbl);
    side_head.append(&new_session_btn);
    let side_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&session_list)
        .build();
    let sidebar = Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(6)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(8)
        .margin_end(8)
        .width_request(196)
        .build();
    sidebar.append(&side_head);
    sidebar.append(&side_scroll);

    // ---- 模型选择（v1.0.17）：对话页内直接下拉本地已装模型，不必绕道「模型管理」页 ----
    let model_combo = ComboBoxText::builder()
        .hexpand(true)
        .tooltip_text("选择本地已装模型")
        .build();
    let model_reload_btn = gtk4::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("刷新模型列表")
        .css_classes(vec!["flat"])
        .build();
    let model_bar = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .build();
    model_bar.append(&model_combo);
    model_bar.append(&model_reload_btn);
    // v3.1 重设计：「参数」从底部操作行提权到顶栏（与模型选择同层级）
    model_bar.append(&params_btn);

    // v3.1 重设计：标题由模型下拉与侧栏承担，不再单占一行
    container.append(&model_bar);
    container.append(&history_scroll);

    // v3.2.0：流式生成状态条（默认隐藏，生成中显示）
    let status_lbl = gtk4::Label::builder()
        .label("生成中…")
        .halign(gtk4::Align::Start)
        .css_classes(vec!["dim-label"])
        .build();
    let status_spin = gtk4::Spinner::new();
    status_spin.start();
    let status_row = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .build();
    status_row.append(&status_spin);
    status_row.append(&status_lbl);
    status_row.set_visible(false);
    container.append(&status_row);
    container.append(&opt_row);
    container.append(&schema_scroll);
    container.append(&input_frame);
    container.append(&send_row);

    // v3.1 重设计：左会话侧栏 + 右对话主区，横向组成对话页根容器
    // v3.2.1：会话侧栏与对话区改用 Paned 分栏——
    // ① 侧栏初始固定 224px（shrink=false 禁止被挤压）；
    // ② 中间分界线可手动拖动调节两侧比例；
    // ③ Paned 记录的是绝对像素位置，窗口缩放时侧栏不再随比例跑动。
    container.set_hexpand(true);
    let chat_root = gtk4::Paned::builder()
        .orientation(Orientation::Horizontal)
        .wide_handle(true)
        .position(224)
        .build();
    chat_root.set_start_child(Some(&sidebar));
    chat_root.set_end_child(Some(&container));
    chat_root.set_shrink_start_child(false);

    let pane = Rc::new(RefCell::new(ChatPane {
        history,
        input,
        model: RefCell::new(None),
        streaming: RefCell::new(false),
        messages: RefCell::new(Vec::new()),
        params: RefCell::new(GenParams {
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            num_ctx: None,
            num_predict: None,
            seed: None,
            repeat_penalty: None,
            stop: None,
            min_p: None,
            typical_p: None,
            presence_penalty: None,
            frequency_penalty: None,
            penalize_newline: None,
            numa: None,
            num_gpu: None,
            main_gpu: None,
            num_batch: None,
            draft_num_predict: None,
            keep_alive: None,
            think: None,
            format: None,
            tools: None,
        }),
        images: RefCell::new(Vec::new()),
        cancel: RefCell::new(None),
        sessions: RefCell::new(Vec::new()),
        current: RefCell::new(0),
        session_list: session_list.clone(),
        model_combo: model_combo.clone(),
        suppress_combo: RefCell::new(false),
        title: title.clone(),
        container: container.clone(),
        status_row: status_row.clone(),
        input_view,
        win: parent.clone(),
        stop_btn: stop_btn.clone(),
        send_btn: send_btn.clone(),
        regen_btn: regen_btn.clone(),
    }));

    // ---- 多会话：启动时加载持久化会话并恢复到活动态 ----
    {
        let (sessions, cur) = load_sessions();
        {
            let p = pane.borrow_mut();
            *p.sessions.borrow_mut() = sessions;
            // 防止索引越界（文件损坏/被手动改动时回落到 0）
            let n = p.sessions.borrow().len();
            let idx = if n == 0 { 0 } else { cur.min(n - 1) };
            *p.current.borrow_mut() = idx;
            let sess = p.sessions.borrow().get(idx).cloned();
            if let Some(s) = sess {
                *p.model.borrow_mut() = s.model.clone();
                *p.messages.borrow_mut() = s.messages.clone();
                if let Some(m) = &s.model {
                    p.title.set_label(&format!("对话模型：{m}"));
                }
            }
        }
        refresh_combo(&pane);
        render_history(&pane);
        save_sessions(&pane);
    }

    // ---- 模型下拉（v1.0.17）：选中 → set_model；刷新按钮重拉本地列表 ----
    {
        let pane_c = Rc::clone(&pane);
        model_combo.connect_changed(move |c| {
            if *pane_c.borrow().suppress_combo.borrow() {
                return;
            }
            if let Some(txt) = c.active_text() {
                let t = txt.to_string();
                if t.is_empty() {
                    return;
                }
                if pane_c.borrow().model.borrow().as_deref() != Some(t.as_str()) {
                    set_model(&pane_c.borrow(), &t);
                }
            }
        });
    }
    {
        let pane_c = Rc::clone(&pane);
        model_reload_btn.connect_clicked(move |_| refresh_model_combo(&pane_c));
    }
    refresh_model_combo(&pane);

    // 侧栏切换会话（程序化刷新列表时 suppress_combo 为 true，避免递归）
    {
        let pane_c = Rc::clone(&pane);
        pane.borrow().session_list.connect_selected_rows_changed(
            move |_| {
                if *pane_c.borrow().suppress_combo.borrow() {
                    return;
                }
                let list = pane_c.borrow().session_list.clone();
                if let Some(row) = list.selected_row() {
                    switch_to(&pane_c, row.index() as usize);
                }
            },
        );
    }
    // 新建会话
    {
        let pane_c = Rc::clone(&pane);
        new_session_btn.connect_clicked(move |_| new_session(&pane_c));
    }

    // ---- 选项行控件接线：图片 / 思考 / 格式 ----
    {
        let p = Rc::clone(&pane);
        let cur = p.borrow().params.borrow().clone();
        think_sw.set_active(cur.think.unwrap_or(false));
        // 输出格式初始化：纯文本 / json / 自定义 JSON Schema
        match cur.format.as_deref() {
            Some("json") => fmt_dd.set_selected(1),
            Some(s) if !s.is_empty() => {
                fmt_dd.set_selected(2);
                schema_buf.set_text(s);
                schema_scroll.set_visible(true);
            }
            _ => fmt_dd.set_selected(0),
        }

        // 图片附件：选多张，读出后转 base64 暂存到 pane.images，随本条消息发送
        let img_lbl_c = img_lbl.clone();
        img_btn.connect_clicked(move |_| {
            let d = gtk4::FileChooserNative::new(
                Some("选择图片"),
                Some(&p.borrow().win),
                gtk4::FileChooserAction::Open,
                Some("打开"),
                Some("取消"),
            );
            d.set_select_multiple(true);
            // v3.1.1 修复：FileDialogNative 在 show 后必须持有引用，否则 GTK 会在
            // 对话框可见期间销毁它导致段错误闪退。把克隆引用移进 response 闭包保活。
            let d_keep = d.clone();
            let p2 = Rc::clone(&p);
            let lbl = img_lbl_c.clone();
            d.connect_response(move |chooser, resp| {
                let _alive = &d_keep;
                if resp != gtk4::ResponseType::Accept {
                    return;
                }
                let files = chooser.files();
                let win = p2.borrow().win.clone();
                for i in 0..files.n_items() {
                    if let Some(obj) = files.item(i) {
                        if let Ok(file) = obj.downcast::<gtk4::gio::File>() {
                            if let Some(path) = file.path() {
                                match std::fs::read(&path) {
                                    Ok(bytes) => {
                                        p2.borrow().images.borrow_mut().push(to_base64(&bytes))
                                    }
                                    Err(e) => crate::ui::dialogs::error(&win, "读取图片失败", &e.to_string()),
                                }
                            }
                        }
                    }
                }
                let n = p2.borrow().images.borrow().len();
                lbl.set_label(&format!("已附 {n} 张"));
            });
            d.show();
        });

        // 思考开关：推理模型是否输出思考过程（用生成的 connect_active_notify，避免 Send 约束）
        let p_think = Rc::clone(&pane);
        think_sw.connect_active_notify(move |sw| {
            p_think.borrow_mut().params.borrow_mut().think = Some(sw.is_active());
        });

        // 输出格式：纯文本 / JSON / 自定义 JSON Schema
        let p_fmt = Rc::clone(&pane);
        let schema_buf_f = schema_buf.clone();
        let schema_scroll_f = schema_scroll.clone();
        fmt_dd.connect_selected_notify(move |dd| {
            let sel = dd.selected();
            let pmut = p_fmt.borrow_mut();
            let mut params = pmut.params.borrow_mut();
            match sel {
                1 => {
                    params.format = Some("json".to_string());
                    schema_scroll_f.set_visible(false);
                }
                2 => {
                    schema_scroll_f.set_visible(true);
                    let t = schema_buf_f
                        .text(&schema_buf_f.start_iter(), &schema_buf_f.end_iter(), false)
                        .to_string();
                    params.format = if t.trim().is_empty() { None } else { Some(t) };
                }
                _ => {
                    params.format = None;
                    schema_scroll_f.set_visible(false);
                }
            }
        });

        // 自定义 Schema 文本变化时实时写回格式（仅在选择「自定义 Schema」时）
        let p_schema = Rc::clone(&pane);
        let fmt_dd_s = fmt_dd.clone();
        let schema_buf_c = schema_buf.clone();
        schema_buf.connect_changed(move |_| {
            if fmt_dd_s.selected() == 2 {
                let t = schema_buf_c
                    .text(&schema_buf_c.start_iter(), &schema_buf_c.end_iter(), false)
                    .to_string();
                let pmut = p_schema.borrow_mut();
                let mut params = pmut.params.borrow_mut();
                params.format = if t.trim().is_empty() { None } else { Some(t) };
            }
        });
    }

    // 发送按钮事件
    let pane2 = Rc::clone(&pane);
    send_btn.connect_clicked(move |_| {
        send_message(&pane2);
    });

    // 停止生成：置位取消标志，后台读取后中断并断开连接
    {
        let pane5 = Rc::clone(&pane);
        stop_btn.connect_clicked(move |_| {
            let p = pane5.borrow();
            if let Some(c) = p.cancel.borrow().as_ref() {
                c.store(true, Ordering::SeqCst);
            }
            p.stop_btn.set_sensitive(false);
        });
    }

    // 清空当前对话
    {
        let pane6 = Rc::clone(&pane);
        clear_btn.connect_clicked(move |_| {
            clear(&pane6);
        });
    }

    // 导出对话
    {
        let pane7 = Rc::clone(&pane);
        export_btn.connect_clicked(move |_| {
            export_conversation(&pane7);
        });
    }

    // 重新生成
    {
        let pane8 = Rc::clone(&pane);
        regen_btn.connect_clicked(move |_| {
            regenerate_inner(&pane8);
        });
    }

    // 参数按钮：编辑生成参数（system / temperature / top_p 等），写入 pane.params
    {
        let pane4 = Rc::clone(&pane);
        params_btn.connect_clicked(move |_| {
            let p = pane4.borrow();
            let cur = p.params.borrow().clone();
            let win = p.win.clone();
            drop(p);
            crate::ui::dialogs::gen_params(&win, &cur, {
                let pane = Rc::clone(&pane4);
                move |np| {
                    let p = pane.borrow();
                    *p.params.borrow_mut() = np;
                }
            });
        });
    }

    // 键盘：Enter 发送，Shift+Enter 换行，Esc 中断生成
    let pane3 = Rc::clone(&pane);
    let key_ctrl = gtk4::EventControllerKey::new();
    key_ctrl.connect_key_pressed(move |_, keyval, _, state| {
        let is_enter = keyval == gdk::Key::Return || keyval == gdk::Key::KP_Enter;
        if is_enter && !state.contains(gdk::ModifierType::SHIFT_MASK) {
            send_message(&pane3);
            return glib::Propagation::Stop;
        }
        // Esc 中断当前生成（与「停止」按钮等价）
        if keyval == gdk::Key::Escape {
            let p = pane3.borrow();
            if *p.streaming.borrow() {
                if let Some(c) = p.cancel.borrow().as_ref() {
                    c.store(true, Ordering::SeqCst);
                }
                p.stop_btn.set_sensitive(false);
                return glib::Propagation::Stop;
            }
        }
        glib::Propagation::Proceed
    });
    {
        let p = pane.borrow();
        p.input_view.add_controller(key_ctrl);
    }

    (chat_root.into(), pane)
}

// ===========================================================================
// 多会话持久化辅助函数
// ===========================================================================

/// 会话持久化文件路径：~/.config/offideskollama/sessions.json
fn sessions_path() -> std::path::PathBuf {
    let dir = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::Path::new(&dir)
        .join(".config")
        .join("offideskollama")
        .join("sessions.json")
}

/// 生成会话唯一 id（毫秒时间戳，足够区分本地会话）
fn new_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("s{ms}")
}

/// 读取持久化会话；无文件或文件损坏则回落到一个空会话
fn load_sessions() -> (Vec<Session>, usize) {
    let path = sessions_path();
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(f) = serde_json::from_str::<SessionsFile>(&s) {
            if !f.sessions.is_empty() {
                return (f.sessions, f.current);
            }
        }
    }
    (
        vec![Session {
            id: new_session_id(),
            title: "会话 1".into(),
            model: None,
            messages: Vec::new(),
        }],
        0,
    )
}

/// 把当前活动态（model / messages）写回 sessions[current]
fn snapshot_current(pane: &Rc<RefCell<ChatPane>>) {
    let p = pane.borrow();
    let idx = *p.current.borrow();
    let mut sessions = p.sessions.borrow_mut();
    if let Some(s) = sessions.get_mut(idx) {
        s.model = p.model.borrow().clone();
        s.messages = p.messages.borrow().clone();
    }
}

/// 持久化全部会话到磁盘
fn save_sessions(pane: &Rc<RefCell<ChatPane>>) {
    snapshot_current(pane);
    let (sessions, current) = {
        let p = pane.borrow();
        let sessions = p.sessions.borrow().clone();
        let current = *p.current.borrow();
        (sessions, current)
    };
    let data = SessionsFile { sessions, current };
    let path = sessions_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string_pretty(&data) {
        let _ = std::fs::write(&path, s);
    }
}

/// 重建下拉框条目并选中当前会话（程序化改动期间抑制 changed 回调）
/// 重建会话侧栏列表（v3.1：ListBox，双行显示标题 + 模型，行内删除按钮）
fn refresh_combo(pane: &Rc<RefCell<ChatPane>>) {
    {
        let p = pane.borrow();
        *p.suppress_combo.borrow_mut() = true;
    }
    let (sessions, cur, list) = {
        let p = pane.borrow();
        let sessions = p.sessions.borrow().clone();
        let cur = *p.current.borrow();
        (sessions, cur, p.session_list.clone())
    };
    while let Some(c) = list.first_child() {
        list.remove(&c);
    }
    for (i, s) in sessions.iter().enumerate() {
        let title = if s.title.trim().is_empty() {
            format!("会话 {}", i + 1)
        } else {
            s.title.clone()
        };
        let sub = s.model.clone().unwrap_or_else(|| "未选择模型".into());

        let vb = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .build();
        let tl = gtk4::Label::builder()
            .label(&title)
            .halign(gtk4::Align::Start)
            .ellipsize(gdk::pango::EllipsizeMode::End)
            .build();
        let sl = gtk4::Label::builder()
            .label(&sub)
            .halign(gtk4::Align::Start)
            .ellipsize(gdk::pango::EllipsizeMode::End)
            .css_classes(vec!["dim-label", "caption"])
            .build();
        vb.append(&tl);
        vb.append(&sl);

        // 行内删除按钮（破坏性操作，轻量图标）
        let del = gtk4::Button::from_icon_name("edit-delete-symbolic");
        del.set_tooltip_text(Some("删除该会话"));
        del.set_css_classes(&["flat"]);
        del.set_valign(gtk4::Align::Center);
        let pane_d = Rc::clone(pane);
        del.connect_clicked(move |_| delete_session_at(&pane_d, i));

        let hb = Box::builder().orientation(Orientation::Horizontal).spacing(6).build();
        hb.append(&vb);
        hb.append(&del);
        let row = gtk4::ListBoxRow::builder().child(&hb).build();
        list.append(&row);
        if i == cur {
            list.select_row(Some(&row));
        }
    }
    let p = pane.borrow();
    *p.suppress_combo.borrow_mut() = false;
}

/// 用当前活动会话的 messages 重建对话展示区
fn render_history(pane: &Rc<RefCell<ChatPane>>) {
    let (messages, model) = {
        let p = pane.borrow();
        let messages = p.messages.borrow().clone();
        let model = p.model.borrow().clone();
        (messages, model)
    };
    let hist = pane.borrow().history.clone();
    hist.set_text("");
    let who = model.unwrap_or_else(|| "助手".to_string());
    for m in &messages {
        if m.role == "user" {
            buf_append(&hist, &format!("你：{}\n\n", m.content));
        } else {
            buf_append(&hist, &format!("{who}：\n"));
            let mut it = hist.end_iter();
            crate::ui::markdown::insert_markdown(&hist, &mut it, &m.content);
            buf_append(&hist, "\n\n");
        }
    }
}

/// 切换到指定会话（先保存当前，再载入目标）
fn switch_to(pane: &Rc<RefCell<ChatPane>>, idx: usize) {
    {
        let p = pane.borrow();
        if idx >= p.sessions.borrow().len() || idx == *p.current.borrow() {
            return;
        }
    }
    snapshot_current(pane);
    {
        let p = pane.borrow();
        *p.current.borrow_mut() = idx;
        let sess = p.sessions.borrow().get(idx).cloned();
        if let Some(s) = sess {
            *p.model.borrow_mut() = s.model.clone();
            *p.messages.borrow_mut() = s.messages.clone();
            *p.images.borrow_mut() = Vec::new();
            if let Some(m) = &s.model {
                p.title.set_label(&format!("对话模型：{m}"));
            } else {
                p.title.set_label("选择左侧模型开始对话");
            }
        }
    }
    refresh_combo(pane);
    render_history(pane);
    save_sessions(pane);
}

/// 新建会话（保存当前，追加空会话并切换进去）
pub fn new_session(pane: &Rc<RefCell<ChatPane>>) {
    snapshot_current(pane);
    {
        let p = pane.borrow();
        if *p.streaming.borrow() {
            return;
        }
        let mut sessions = p.sessions.borrow_mut();
        let idx = sessions.len();
        sessions.push(Session {
            id: new_session_id(),
            title: format!("会话 {}", idx + 1),
            model: None,
            messages: Vec::new(),
        });
        *p.current.borrow_mut() = idx;
        *p.model.borrow_mut() = None;
        *p.messages.borrow_mut() = Vec::new();
        *p.images.borrow_mut() = Vec::new();
        p.history.set_text("");
        p.title.set_label("选择左侧模型开始对话");
    }
    refresh_combo(pane);
    save_sessions(pane);
}

/// 重命名当前会话
pub fn rename_current(pane: &Rc<RefCell<ChatPane>>) {
    let cur_title = {
        let p = pane.borrow();
        let idx = *p.current.borrow();
        let sessions = p.sessions.borrow();
        sessions.get(idx).map(|s| s.title.clone()).unwrap_or_default()
    };
    let win = pane.borrow().win.clone();
    let pane_c = Rc::clone(pane);
    crate::ui::dialogs::text_input(&win, "重命名会话", "会话名称", &cur_title, "确定", move |opt| {
        if let Some(name) = opt {
            let name = name.trim().to_string();
            if !name.is_empty() {
                let p = pane_c.borrow();
                let idx = *p.current.borrow();
                if let Some(s) = p.sessions.borrow_mut().get_mut(idx) {
                    s.title = name;
                }
                drop(p);
                refresh_combo(&pane_c);
                save_sessions(&pane_c);
            }
        }
    });
}

/// 删除当前会话（仅剩一个时改为清空而非删除）
pub fn delete_current(pane: &Rc<RefCell<ChatPane>>) {
    let idx = *pane.borrow().current.borrow();
    delete_session_at(pane, idx);
}

/// 删除指定索引的会话（v3.1 侧栏行内删除；仅剩一个时改为清空而非删除）
pub fn delete_session_at(pane: &Rc<RefCell<ChatPane>>, idx: usize) {
    {
        let p = pane.borrow();
        if *p.streaming.borrow() {
            return;
        }
        let n = p.sessions.borrow().len();
        let idx = idx.min(n.saturating_sub(1));
        if n <= 1 {
            if let Some(s) = p.sessions.borrow_mut().get_mut(0) {
                s.messages.clear();
                s.model = None;
                s.title = "会话 1".into();
            }
            *p.current.borrow_mut() = 0;
            *p.model.borrow_mut() = None;
            *p.messages.borrow_mut() = Vec::new();
            *p.images.borrow_mut() = Vec::new();
            p.history.set_text("");
            p.title.set_label("选择左侧模型开始对话");
        } else {
            p.sessions.borrow_mut().remove(idx);
            let new_idx = idx.min(n - 2); // 删除后长度 n-1，最大有效索引 n-2
            *p.current.borrow_mut() = new_idx;
            if let Some(s) = p.sessions.borrow().get(new_idx).cloned() {
                *p.model.borrow_mut() = s.model.clone();
                *p.messages.borrow_mut() = s.messages.clone();
                *p.images.borrow_mut() = Vec::new();
                if let Some(m) = &s.model {
                    p.title.set_label(&format!("对话模型：{m}"));
                } else {
                    p.title.set_label("选择左侧模型开始对话");
                }
            }
        }
    }
    refresh_combo(pane);
    render_history(pane);
    save_sessions(pane);
}

/// 设置当前模型：更新标题与当前会话元数据，**不再清空历史**
/// （设计硬伤修复：换模型保留会话，每条会话可绑定不同模型）
pub fn set_model(pane: &ChatPane, model: &str) {
    *pane.model.borrow_mut() = Some(model.to_string());
    pane.title.set_label(&format!("对话模型：{model}"));
    let idx = *pane.current.borrow();
    if let Some(s) = pane.sessions.borrow_mut().get_mut(idx) {
        s.model = Some(model.to_string());
    }
    // 同步模型下拉框（v1.0.17）：suppress_combo 防止 changed 信号回环。
    // append 时以模型名为 id，直接用 active-id 属性匹配选中项。
    *pane.suppress_combo.borrow_mut() = true;
    pane.model_combo.set_property("active-id", model.to_string());
    *pane.suppress_combo.borrow_mut() = false;
}

/// 拉取本地已装模型填充对话页模型下拉（v1.0.17）。
/// 已选中模型保持选中；无会话模型时自动选第一个，省一步操作。
pub fn refresh_model_combo(pane: &Rc<RefCell<ChatPane>>) {
    let client = crate::ollama::client();
    let p = Rc::clone(pane);
    crate::rt::spawn(
        async move {
            match crate::ollama::models::list_models(&client).await {
                Ok(m) => m.into_iter().map(|m| m.name).collect::<Vec<_>>(),
                Err(_) => Vec::new(),
            }
        },
        move |names: Vec<String>| {
            let current = p.borrow().model.borrow().clone();
            {
                let pc = p.borrow();
                *pc.suppress_combo.borrow_mut() = true;
                pc.model_combo.remove_all();
                if names.is_empty() {
                    pc.model_combo.append(Some(""), "（未连接服务或无已装模型）");
                } else {
                    for n in &names {
                        pc.model_combo.append(Some(n), n);
                    }
                }
                *pc.suppress_combo.borrow_mut() = false;
            }
            if names.is_empty() {
                return;
            }
            // 恢复选中：优先当前模型，否则自动选第一个
            let sel = current
                .as_deref()
                .and_then(|c| names.iter().position(|n| n == c))
                .unwrap_or(0);
            {
                let pc = p.borrow();
                *pc.suppress_combo.borrow_mut() = true;
                pc.model_combo.set_active(Some(sel as u32));
                *pc.suppress_combo.borrow_mut() = false;
            }
            if current.is_none() {
                let first = names[0].clone();
                set_model(&p.borrow(), &first);
            }
        },
    );
}

/// 清空当前对话（菜单栏「会话 → 清空」复用）
pub fn clear(pane: &Rc<RefCell<ChatPane>>) {
    let p = pane.borrow();
    if *p.streaming.borrow() {
        return;
    }
    p.messages.borrow_mut().clear();
    p.images.borrow_mut().clear();
    p.history.set_text("");
    drop(p);
    // 同步清空当前会话的持久化内容
    {
        let p = pane.borrow();
        let idx = *p.current.borrow();
        let mut sessions = p.sessions.borrow_mut();
        if let Some(s) = sessions.get_mut(idx) {
            s.messages.clear();
        }
    }
    save_sessions(pane);
}


/// 导出对话为 Markdown（菜单栏「会话 → 导出」复用）
pub fn export(pane: &Rc<RefCell<ChatPane>>) {
    export_conversation(pane);
}

/// 重新生成上一条回复（菜单栏「会话 → 重新生成」复用）
pub fn regenerate(pane: &Rc<RefCell<ChatPane>>) {
    regenerate_inner(pane);
}

/// 获取输入框文本
pub fn get_input(pane: &ChatPane) -> String {
    let buf = &pane.input;
    buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string()
}

/// 发送消息：追加用户消息 → 后台流式 → 逐 token 更新助手回复。
/// 完整多轮上下文（messages 会累积用户与助手消息）。
fn send_message(pane: &Rc<RefCell<ChatPane>>) {
    let text;
    {
        let p = pane.borrow();
        if *p.streaming.borrow() {
            return;
        }
        let t = p
            .input
            .text(&p.input.start_iter(), &p.input.end_iter(), false)
            .to_string();
        let t = t.trim().to_string();
        if t.is_empty() {
            return;
        }
        if p.model.borrow().is_none() {
            buf_append(&p.history, "请先在左侧选择模型\n");
            return;
        }
        // v1.0.10：仅嵌入模型不支持对话，发送前拦截（能力未探测到时不拦，靠报错翻译兜底）
        if let Some(m) = p.model.borrow().as_ref() {
            if crate::ollama::models::cached_is_embed_only(m) == Some(true) {
                buf_append(
                    &p.history,
                    &format!(
                        "\n{m} 是嵌入（embedding）模型，不支持对话。\n请在左侧选择对话模型（如 qwen2.5、llama3.2）；嵌入模型请在「嵌入」页使用。\n"
                    ),
                );
                return;
            }
        }
        text = t;
        // 清空输入，追加用户消息到记录
        p.input.set_text("");
        buf_append(&p.history, &format!("\n你：{text}\n\n"));
        // 推入上下文
        p.messages.borrow_mut().push(ChatMessage {
            role: "user".into(),
            content: text.clone(),
        });
    }
    // 用首条用户消息命名会话（仅当仍是默认名），并持久化
    {
        let p = pane.borrow();
        let idx = *p.current.borrow();
        let is_default = p
            .sessions
            .borrow()
            .get(idx)
            .map(|s| s.title.starts_with("会话 "))
            .unwrap_or(false);
        if is_default {
            let title = text.chars().take(14).collect::<String>();
            if let Some(s) = p.sessions.borrow_mut().get_mut(idx) {
                s.title = if title.is_empty() {
                    format!("会话 {}", idx + 1)
                } else {
                    title
                };
            }
            refresh_combo(pane);
        }
    }
    save_sessions(pane);
    start_stream(pane);
    // 图片为一次性附件，发送后即清空（不影响已发出的消息）
    pane.borrow().images.borrow_mut().clear();
}

/// 重新生成上一条回复：丢弃尾部助手消息，用同一条用户消息再跑一次
fn regenerate_inner(pane: &Rc<RefCell<ChatPane>>) {
    {
        let p = pane.borrow();
        if *p.streaming.borrow() || p.model.borrow().is_none() {
            return;
        }
        let mut msgs = p.messages.borrow_mut();
        // 去掉尾部所有助手回复（含失败残留），回到最后一条用户消息
        while matches!(msgs.last(), Some(m) if m.role == "assistant") {
            msgs.pop();
        }
        if !matches!(msgs.last(), Some(m) if m.role == "user") {
            return;
        }
    }
    {
        let p = pane.borrow();
        buf_append(&p.history, "\n（重新生成）\n");
    }
    start_stream(pane);
}

/// 导出对话为 Markdown 文件
fn export_conversation(pane: &Rc<RefCell<ChatPane>>) {
    // 逐个取值，避免在同一表达式里嵌套借用导致生命周期冲突
    let msgs = pane.borrow().messages.borrow().clone();
    let model = pane.borrow().model.borrow().clone();
    let win = pane.borrow().win.clone();
    if msgs.is_empty() {
        return;
    }

    let dialog = gtk4::FileChooserNative::new(
        Some("导出对话"),
        Some(&win),
        gtk4::FileChooserAction::Save,
        Some("保存"),
        Some("取消"),
    );
    dialog.set_current_name("conversation.md");

    // v3.1.1：同图片对话框，克隆引用进闭包保活，防 show 后销毁段错误
    let pane_c = Rc::clone(pane);
    let dialog_keep = dialog.clone();
    dialog.connect_response(move |d, resp| {
        let _alive = &dialog_keep;
        if resp != gtk4::ResponseType::Accept {
            return;
        }
        let Some(file) = d.file() else { return };
        let Some(path) = file.path() else { return };

        let mut md = String::from("# offideskOllama 对话导出\n\n");
        if let Some(m) = &model {
            md.push_str(&format!("模型：{m}\n\n"));
        }
        for m in &msgs {
            let who = if m.role == "user" { "你" } else { "助手" };
            md.push_str(&format!("**{who}**\n\n{}\n\n", m.content));
        }

        let p = pane_c.borrow();
        match std::fs::write(&path, md) {
            // 如实反馈结果
            Ok(()) => crate::ui::dialogs::info(
                &p.win,
                "已导出",
                &format!("对话已保存到 {}", path.display()),
            ),
            Err(e) => crate::ui::dialogs::error(&p.win, "导出失败", &e.to_string()),
        }
    });
    dialog.show();
}

/// 发起一次流式生成（发送新消息与「重新生成」共用同一条路径）
fn start_stream(pane: &Rc<RefCell<ChatPane>>) {
    // 逐个取值，避免在同一表达式里嵌套借用导致生命周期冲突
    let model = match pane.borrow().model.borrow().as_ref() {
        Some(m) => m.clone(),
        None => {
            buf_append(&pane.borrow().history, "请先在左侧选择模型\n");
            return;
        }
    };
    let messages = pane.borrow().messages.borrow().clone();
    let params = pane.borrow().params.borrow().clone();
    let images = pane.borrow().images.borrow().clone();

    // 本次生成的取消标志；「停止」按钮置位后后台会中断读取并断开连接
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let p = pane.borrow();
        *p.cancel.borrow_mut() = Some(cancel.clone());
        *p.streaming.borrow_mut() = true;
        p.status_row.set_visible(true);
        p.stop_btn.set_sensitive(true);
        p.send_btn.set_sensitive(false);
        p.regen_btn.set_sensitive(false);
    }

    // 流式共享状态：是否已插入模型回复标题、累计的原始回复、回复起始偏移、思考过程
    let header_done = Rc::new(RefCell::new(false));
    let reply_buf = Rc::new(RefCell::new(String::new()));
    let reply_start = Rc::new(RefCell::new(None::<usize>));
    let think_buf = Rc::new(RefCell::new(String::new()));
    let think_header = Rc::new(RefCell::new(false));

    let on_token = {
        let pane = Rc::clone(pane);
        let header_done = Rc::clone(&header_done);
        let reply_buf = Rc::clone(&reply_buf);
        let reply_start = Rc::clone(&reply_start);
        let think_buf = Rc::clone(&think_buf);
        let think_header = Rc::clone(&think_header);
        let m = model.clone();
        move |msg: StreamMsg| {
            match msg {
                StreamMsg::Value(tok) => {
                    let p = pane.borrow();
                    if !*header_done.borrow() {
                        *header_done.borrow_mut() = true;
                        buf_append(&p.history, &format!("{m}：\n"));
                        // 若已有思考过程，用分隔线与正文隔开
                        if !think_buf.borrow().is_empty() {
                            buf_append(&p.history, "—\n");
                        }
                        // 记录助手回复正文起点，收尾时从这里重渲染为 Markdown
                        *reply_start.borrow_mut() = Some(p.history.char_count() as usize);
                    }
                    buf_append(&p.history, &tok);
                    reply_buf.borrow_mut().push_str(&tok);
                }
                StreamMsg::Think(tok) => {
                    let p = pane.borrow();
                    if !*think_header.borrow() {
                        *think_header.borrow_mut() = true;
                        buf_append(&p.history, "\n💭 思考过程：\n");
                    }
                    buf_append(&p.history, &tok);
                    think_buf.borrow_mut().push_str(&tok);
                }
                StreamMsg::Done(_) => {}
            }
        }
    };

    let on_done = {
        let pane = Rc::clone(pane);
        let reply_buf = Rc::clone(&reply_buf);
        let reply_start = Rc::clone(&reply_start);
        move |msg: StreamMsg| {
            let StreamMsg::Done(res) = msg else { return };
            let p = pane.borrow();
            let reply = reply_buf.borrow().clone();

            // 把流式阶段追加的纯文本重写为 Markdown 富文本
            if let Some(start_off) = *reply_start.borrow() {
                let mut start = p.history.iter_at_offset(start_off as i32);
                let mut end = p.history.end_iter();
                p.history.delete(&mut start, &mut end);
                crate::ui::markdown::insert_markdown(&p.history, &mut start, &reply);
            }

            let mut tail = p.history.end_iter();
            match res {
                Ok(stats) => {
                    // 函数调用结果：模型请求调用外部函数时整理成可读文本（展示 + 持久化）
                    let tool_text = stats.tool_calls_text();
                    let full_content = if tool_text.is_empty() {
                        reply.clone()
                    } else if reply.is_empty() {
                        tool_text.clone()
                    } else {
                        format!("{reply}\n{tool_text}")
                    };
                    if !full_content.is_empty() {
                        p.messages.borrow_mut().push(ChatMessage {
                            role: "assistant".into(),
                            content: full_content.clone(),
                        });
                    }
                    // 如实呈现速度与耗时（对应 `ollama run --verbose` 的口径）
                    let summary = stats.summary();
                    if !summary.is_empty() {
                        crate::ui::markdown::insert_tagged(
                            &p.history,
                            &mut tail,
                            &format!("\n{summary}\n"),
                            "md-meta",
                        );
                    } else if full_content.is_empty() {
                        crate::ui::markdown::insert_tagged(
                            &p.history,
                            &mut tail,
                            "\n（已中断，无内容）\n",
                            "md-meta",
                        );
                    }
                    // 函数调用以主题色块展示，与正常输出明显区分
                    if !tool_text.is_empty() {
                        crate::ui::markdown::insert_tagged(
                            &p.history,
                            &mut tail,
                            &format!("\n{tool_text}\n"),
                            "md-tool",
                        );
                    }
                }
                Err(e) => {
                    // 失败必须显式呈现，不能静默吞掉；能力错误翻译成可行动提示（v1.0.10）
                    let shown = crate::ollama::models::friendly_capability_error(&e)
                        .unwrap_or_else(|| format!("生成失败：{e}"));
                    crate::ui::markdown::insert_tagged(
                        &p.history,
                        &mut tail,
                        &format!("\n{shown}\n"),
                        "md-err",
                    );
                }
            }
            buf_append(&p.history, "\n");

            *p.streaming.borrow_mut() = false;
            p.status_row.set_visible(false);
            *p.cancel.borrow_mut() = None;
            p.stop_btn.set_sensitive(false);
            p.send_btn.set_sensitive(true);
            p.regen_btn.set_sensitive(true);
            // 生成完成：把本轮对话（含助手回复）持久化到磁盘
            drop(p);
            save_sessions(&pane);
        }
    };

    let client = ollama::client();
    let cancel_bg = cancel.clone();
    let images_bg = images.clone();
    rt::spawn_stream(
        move |tx| {
            crate::rt::block_on(async move {
                let res = ollama::chat::stream_chat(
                    &client,
                    &model,
                    &messages,
                    &params,
                    &images_bg,
                    cancel_bg,
                    |tok| {
                        let _ = tx.send(StreamMsg::Value(tok.to_string()));
                    },
                    |tok| {
                        let _ = tx.send(StreamMsg::Think(tok.to_string()));
                    },
                )
                .await;
                let _ = tx.send(StreamMsg::Done(res.map_err(|e| e.to_string())));
            });
        },
        on_token,
        on_done,
    );
}
