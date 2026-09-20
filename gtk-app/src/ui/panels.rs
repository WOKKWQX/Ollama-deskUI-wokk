//! 主区 Tab 面板：单次生成、已加载模型、文本嵌入
//! 每个面板返回 (widget, state句柄)，state 供主框架刷新/联动。

use crate::ollama::{self, GenParams};
use crate::rt;
use crate::ui::dialogs;
use gtk4::prelude::*;
use gtk4::{Box, Label, Orientation, ScrolledWindow, TextBuffer, TextView};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 标准 Base64 编码（多模态图片送 Ollama 时转 base64，避免引入额外依赖）
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

// ---------------------------------------------------------------------------
// 单次生成（/api/generate）
// ---------------------------------------------------------------------------
pub struct GeneratePanel {
    pub input: TextBuffer,
    pub output: TextBuffer,
    pub model: RefCell<String>,
    pub busy: RefCell<bool>,
    /// 本次生成的取消标志（点「停止」置位）
    pub cancel: RefCell<Option<Arc<AtomicBool>>>,
    /// 附带的图片（base64，多模态）。发送后清空。
    pub images: RefCell<Vec<String>>,
    win: gtk4::Window,
    gen_btn: gtk4::Button,
    stop_btn: gtk4::Button,
}

pub fn build_generate_panel(parent: &gtk4::Window) -> (Box, Rc<RefCell<GeneratePanel>>) {
    let win = parent.clone();

    let title = Label::builder().label("单次生成").css_classes(vec!["title-3", "heading"]).halign(gtk4::Align::Start).build();

    let input = TextBuffer::builder().build();
    let input_view = TextView::builder().buffer(&input).height_request(100).wrap_mode(gtk4::WrapMode::WordChar).css_classes(vec!["monospace"]).build();
    let input_scroll = ScrolledWindow::builder().hscrollbar_policy(gtk4::PolicyType::Never).min_content_height(100).child(&input_view).build();

    let output = TextBuffer::builder().build();
    // 注册标签，供统计行 / 错误行使用（普通输出不受影响）
    crate::ui::markdown::ensure_md_tags(&output);
    let output_view = TextView::builder().buffer(&output).editable(false).wrap_mode(gtk4::WrapMode::WordChar).css_classes(vec!["monospace"]).build();
    let output_scroll = ScrolledWindow::builder().hscrollbar_policy(gtk4::PolicyType::Never).vexpand(true).child(&output_view).build();

    let hint = Label::builder().label("提示：先选模型，输入文本，点生成。").css_classes(vec!["dim-label"]).halign(gtk4::Align::Start).build();
    let gen_btn = gtk4::Button::builder().label("生成").css_classes(vec!["suggested-action"]).build();
    let stop_btn = gtk4::Button::builder()
        .label("停止")
        .icon_name("media-playback-stop-symbolic")
        .tooltip_text("中断本次生成")
        .css_classes(vec!["destructive-action"])
        .build();
    stop_btn.set_sensitive(false); // 仅生成中可用

    // 多模态图片附件（随本条提示发送，视觉模型可用）
    let img_btn = gtk4::Button::builder()
        .icon_name("image-x-generic-symbolic")
        .tooltip_text("附加图片（多模态，随本条提示发送）")
        .css_classes(vec!["flat"])
        .build();
    let img_lbl = gtk4::Label::builder().label("").css_classes(vec!["dim-label"]).build();
    let opt_row = Box::builder().orientation(Orientation::Horizontal).spacing(8).build();
    opt_row.append(&img_btn);
    opt_row.append(&img_lbl);

    let btn_row = Box::builder().orientation(Orientation::Horizontal).spacing(8).build();
    let btn_spacer = Box::builder().orientation(Orientation::Horizontal).hexpand(true).build();
    btn_row.append(&btn_spacer);
    btn_row.append(&stop_btn);
    btn_row.append(&gen_btn);

    let vbox = Box::builder().orientation(Orientation::Vertical).spacing(8).margin_top(8).margin_bottom(8).margin_start(12).margin_end(12).build();
    vbox.append(&title);
    vbox.append(&hint);
    vbox.append(&opt_row);
    vbox.append(&input_scroll);
    vbox.append(&output_scroll);
    vbox.append(&btn_row);

    let panel = Rc::new(RefCell::new(GeneratePanel {
        input,
        output,
        model: RefCell::new(String::new()),
        busy: RefCell::new(false),
        cancel: RefCell::new(None),
        images: RefCell::new(Vec::new()),
        win: win.clone(),
        gen_btn: gen_btn.clone(),
        stop_btn: stop_btn.clone(),
    }));

    // 停止生成
    {
        let p_stop = Rc::clone(&panel);
        stop_btn.connect_clicked(move |_| {
            let panel = p_stop.borrow();
            if let Some(c) = panel.cancel.borrow().as_ref() {
                c.store(true, Ordering::SeqCst);
            }
            panel.stop_btn.set_sensitive(false);
        });
    }

    // 图片附件：选多张，读出后转 base64 暂存到 panel.images，随本条提示发送
    {
        let p_img = Rc::clone(&panel);
        let img_lbl_c = img_lbl.clone();
        img_btn.connect_clicked(move |_| {
            let d = gtk4::FileChooserNative::new(
                Some("选择图片"),
                Some(&p_img.borrow().win),
                gtk4::FileChooserAction::Open,
                Some("打开"),
                Some("取消"),
            );
            d.set_select_multiple(true);
            // v3.1.1：引用保活，防 show 后销毁段错误（同 chat.rs）
            let d_keep = d.clone();
            let p2 = Rc::clone(&p_img);
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
                                    Ok(bytes) => p2.borrow().images.borrow_mut().push(to_base64(&bytes)),
                                    Err(e) => dialogs::error(&win, "读取图片失败", &e.to_string()),
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
    }

    let p = Rc::clone(&panel);
    let win2 = win.clone();
    gen_btn.connect_clicked(move |_| {
        let images = p.borrow().images.borrow().clone();
        let panel = p.borrow_mut();
        if *panel.busy.borrow() {
            return;
        }
        let model = panel.model.borrow().clone();
        if model.is_empty() {
            let w = win2.clone();
            dialogs::info(&w, "提示", "请先在左侧选择模型");
            return;
        }
        let input = panel.input.text(&panel.input.start_iter(), &panel.input.end_iter(), false).to_string();
        let out = panel.output.clone();
        let input = input.trim().to_string();
        if input.is_empty() {
            return;
        }
        out.set_text("");
        *panel.busy.borrow_mut() = true;

        let cancel = Arc::new(AtomicBool::new(false));
        *panel.cancel.borrow_mut() = Some(cancel.clone());
        panel.gen_btn.set_sensitive(false);
        panel.stop_btn.set_sensitive(true);
        // 图片为一次性附件，发送后即清空
        panel.images.borrow_mut().clear();

        let client = ollama::client();
        let params = GenParams::default();
        let out2 = out.clone();
        let p_token = Rc::clone(&p);
        let p_done = Rc::clone(&p);
        let cancel_bg = cancel.clone();
        let images_bg = images.clone();
        rt::spawn_stream(
            move |tx| {
                crate::rt::block_on(async move {
                    let res = ollama::chat::stream_generate(&client, &model, &input, &params, &images_bg, cancel_bg, |tok| {
                        let _ = tx.send(crate::rt::StreamMsg::Value(tok.to_string()));
                    }, |_tok| {}).await;
                    let _ = tx.send(crate::rt::StreamMsg::Done(res.map_err(|e| e.to_string())));
                });
            },
            move |msg| {
                let _panel = p_token.borrow_mut();
                if let crate::rt::StreamMsg::Value(t) = msg {
                    let mut it = out2.end_iter();
                    out2.insert(&mut it, &t);
                }
            },
            move |msg| {
                let panel = p_done.borrow_mut();
                *panel.busy.borrow_mut() = false;
                *panel.cancel.borrow_mut() = None;
                panel.gen_btn.set_sensitive(true);
                panel.stop_btn.set_sensitive(false);

                // 收尾如实呈现统计或错误
                let mut tail = panel.output.end_iter();
                match msg {
                    crate::rt::StreamMsg::Done(Ok(stats)) => {
                        let s = stats.summary();
                        if !s.is_empty() {
                            crate::ui::markdown::insert_tagged(
                                &panel.output,
                                &mut tail,
                                &format!("\n{s}\n"),
                                "md-meta",
                            );
                        }
                    }
                    crate::rt::StreamMsg::Done(Err(e)) => {
                        // 能力错误翻译成可行动提示（v1.0.10）
                        let shown = crate::ollama::models::friendly_capability_error(&e)
                            .unwrap_or_else(|| format!("生成失败：{e}"));
                        crate::ui::markdown::insert_tagged(
                            &panel.output,
                            &mut tail,
                            &format!("\n{shown}\n"),
                            "md-err",
                        );
                    }
                    _ => {}
                }
            },
        );
    });

    (vbox, panel)
}

impl GeneratePanel {
    pub fn set_model(&self, model: &str) {
        *self.model.borrow_mut() = model.to_string();
    }
}



// ---------------------------------------------------------------------------
// 文本嵌入（/api/embed）
// ---------------------------------------------------------------------------
pub struct EmbedPanel {
    pub input: TextBuffer,
    pub output: TextBuffer,
    pub model: RefCell<String>,
}

pub fn build_embed_panel() -> (Box, Rc<RefCell<EmbedPanel>>) {
    let title = Label::builder().label("文本嵌入").css_classes(vec!["title-3", "heading"]).halign(gtk4::Align::Start).build();
    let hint = Label::builder().label("每行一条文本，可一次嵌入多条；显示每条的维度与样本。").css_classes(vec!["dim-label"]).halign(gtk4::Align::Start).build();

    let input = TextBuffer::builder().build();
    let input_view = TextView::builder().buffer(&input).height_request(90).wrap_mode(gtk4::WrapMode::WordChar).css_classes(vec!["monospace"]).build();
    let input_scroll = ScrolledWindow::builder().hscrollbar_policy(gtk4::PolicyType::Never).min_content_height(90).child(&input_view).build();

    let output = TextBuffer::builder().build();
    let output_view = TextView::builder().buffer(&output).editable(false).wrap_mode(gtk4::WrapMode::WordChar).css_classes(vec!["monospace"]).build();
    let output_scroll = ScrolledWindow::builder().hscrollbar_policy(gtk4::PolicyType::Never).vexpand(true).child(&output_view).build();

    let embed_btn = gtk4::Button::builder().label("嵌入").css_classes(vec!["suggested-action"]).halign(gtk4::Align::End).build();

    let vbox = Box::builder().orientation(Orientation::Vertical).spacing(8).margin_top(8).margin_bottom(8).margin_start(12).margin_end(12).build();
    vbox.append(&title);
    vbox.append(&hint);
    vbox.append(&input_scroll);
    vbox.append(&output_scroll);
    vbox.append(&embed_btn);

    let panel = Rc::new(RefCell::new(EmbedPanel { input, output, model: RefCell::new(String::new()) }));

    let p = Rc::clone(&panel);
    embed_btn.connect_clicked(move |_| {
        let panel = p.borrow();
        let model = panel.model.borrow().clone();
        if model.is_empty() {
            return;
        }
        let raw = panel.input.text(&panel.input.start_iter(), &panel.input.end_iter(), false).to_string();
        let lines: Vec<String> = raw
            .split('\n')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if lines.is_empty() {
            return;
        }
        let out = panel.output.clone();
        out.set_text("");
        let client = ollama::client();
        rt::spawn(
            async move {
                match ollama::chat::embed(&client, &model, &lines).await {
                    Ok(v) => Ok(v),
                    Err(e) => Err(e.to_string()),
                }
            },
            move |res: Result<Vec<Vec<f64>>, String>| {
                match res {
                    Ok(vectors) => {
                        let mut text = String::new();
                        if vectors.is_empty() {
                            text.push_str("未返回向量");
                        }
                        for (i, v) in vectors.iter().enumerate() {
                            let dims = v.len();
                            let sample: Vec<String> = v.iter().take(8).map(|x| format!("{:.4}", x)).collect();
                            text.push_str(&format!(
                                "第 {} 条：维度 {}\n样本前 8 维：{}\n完整（前 32 维）：{}\n\n",
                                i + 1,
                                dims,
                                sample.join(", "),
                                v.iter().take(32).map(|x| format!("{:.4}", x)).collect::<Vec<_>>().join(", ")
                            ));
                        }
                        let mut it = out.end_iter();
                        out.insert(&mut it, &text);
                    }
                    Err(e) => {
                        let mut it = out.end_iter();
                        out.insert(&mut it, &format!("错误：{e}"));
                    }
                }
            },
        );
    });

    (vbox, panel)
}

impl EmbedPanel {
    pub fn set_model(&self, model: &str) {
        *self.model.borrow_mut() = model.to_string();
    }
}
