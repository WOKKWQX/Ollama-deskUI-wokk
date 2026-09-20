//! 概览 · 本地模型运行时驾驶舱（0.17.0 重设计落地）
//!
//! 四张状态卡（服务 / 版本 / 已加载 / 显存）+ 已加载·运行时面板（keep_alive 倒计时、
//! 常驻 / 卸载行内操作）+ 最近会话摘要。全部数据来自统一轮询的真实状态，不伪造。

use gtk4::prelude::*;
use gtk4::*;
use std::rc::Rc;

use crate::ollama;

/// 简单卡片：标题 + 值 + 说明
fn stat_card(title: &str) -> (gtk4::Box, gtk4::Label, gtk4::Label) {
    let card = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(12)
        .margin_end(12)
        .build();
    let t = Label::builder().label(title).css_classes(vec!["dim-label"]).halign(Align::Start).build();
    let v = Label::builder().label("—").halign(Align::Start).build();
    v.set_markup(&format!("<span size='x-large' weight='bold'>—</span>"));
    let s = Label::builder().label(" ").css_classes(vec!["dim-label"]).halign(Align::Start).build();
    card.append(&t);
    card.append(&v);
    card.append(&s);
    (card, v, s)
}

/// 构建概览页；返回页面根容器
pub fn build_overview_page(parent: &gtk4::Window) -> gtk4::Box {
    let root = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(16)
        .margin_end(16)
        .build();

    // ---- 页头：概览（标题 + 一句事实说明）----
    let head = Label::builder()
        .halign(Align::Start)
        .build();
    head.set_markup("<span size='x-large' weight='bold'>概览</span>");
    root.append(&head);

    // ---- 4 张状态卡 ----
    let cards = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .homogeneous(true)
        .build();
    let (c_svc, v_svc, s_svc) = stat_card("服务状态");
    let (c_ver, v_ver, s_ver) = stat_card("Ollama 版本");
    let (c_loaded, v_loaded, s_loaded) = stat_card("已加载模型");
    let (c_vram, v_vram, s_vram) = stat_card("显存占用");
    for c in [&c_svc, &c_ver, &c_loaded, &c_vram] {
        cards.append(c);
    }
    root.append(&cards);

    // ---- 已加载模型 · 运行时 ----
    let loaded_title = Label::builder()
        .label("已加载模型 · 运行时")
        .halign(Align::Start)
        .build();
    loaded_title.set_markup("<b>已加载模型 · 运行时</b>");
    root.append(&loaded_title);

    let loaded_list = gtk4::ListBox::builder().selection_mode(SelectionMode::None).build();
    // 空态提示：不跨轮询复用同一控件实例（重复 parent 会导致 GTK panic），每次重建新 Label
    {
        let hint = Label::builder()
            .label("暂无模型驻留显存 · 首次调用将自动预热（warming）")
            .css_classes(vec!["dim-label"])
            .halign(Align::Start)
            .build();
        let row = ListBoxRow::new();
        row.set_child(Some(&hint));
        loaded_list.append(&row);
    }
    {
        let sc = ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(PolicyType::Never)
            .child(&loaded_list)
            .build();
        root.append(&sc);
    }

    // v1.0.16：移除「最近会话」——只读摘要无法操作、价值存疑，按「精简收敛」原则删除

    // ---- 轮询刷新（3 秒，与全局轮询同源数据）----
    let v_svc = v_svc.clone();
    let s_svc = s_svc.clone();
    let v_ver = v_ver.clone();
    let s_ver = s_ver.clone();
    let v_loaded = v_loaded.clone();
    let s_loaded = s_loaded.clone();
    let v_vram = v_vram.clone();
    let s_vram = s_vram.clone();
    let loaded_list = loaded_list.clone();
    let win = parent.clone();
    let refresh = move || {
        // 重入保护：上一轮未返回则本轮跳过（v1.0.4 卡死修复）
        static POLL_BUSY: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if POLL_BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let v_svc = v_svc.clone();
        let s_svc = s_svc.clone();
        let v_ver = v_ver.clone();
        let s_ver = s_ver.clone();
        let v_loaded = v_loaded.clone();
        let s_loaded = s_loaded.clone();
        let v_vram = v_vram.clone();
        let s_vram = s_vram.clone();
        let loaded_list = loaded_list.clone();
        let win = win.clone();
        crate::rt::spawn(
            async move {
                let up = crate::service::is_running();
                let managed = crate::service::managed_by().to_string();
                let ver = ollama::chat::get_version(&ollama::client()).await.ok();
                let loaded = ollama::models::loaded_models(&ollama::client()).await.unwrap_or_default();
                let total = ollama::models::list_models(&ollama::client()).await.map(|m| m.len()).unwrap_or(0);
                (up, managed, ver, loaded, total)
            },
            move |(up, managed, ver, loaded, total)| {
                POLL_BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
                // 服务卡
                if up {
                    v_svc.set_markup("<span size='x-large' weight='bold' foreground='#33d17a'>● 运行中</span>");
                    s_svc.set_label(&format!("托管方式：{managed}"));
                } else {
                    v_svc.set_markup("<span size='x-large' weight='bold' foreground='#e01b24'>● 未运行</span>");
                    s_svc.set_label("可在「服务」页启动");
                }
                // 版本卡
                match ver {
                    Some(v) => {
                        v_ver.set_markup(&format!("<span size='x-large' weight='bold'>{v}</span>"));
                        s_ver.set_label("API 连接正常");
                    }
                    None => {
                        v_ver.set_markup("<span size='x-large' weight='bold'>—</span>");
                        s_ver.set_label("API 未连接");
                    }
                }
                // 已加载 / 显存卡
                let vram: u64 = loaded.iter().map(|m| m.size_vram).sum();
                v_loaded.set_markup(&format!(
                    "<span size='x-large' weight='bold'>{}</span>",
                    if loaded.is_empty() { "0".to_string() } else { format!("{}", loaded.len()) }
                ));
                s_loaded.set_label(&format!("本地共 {total} 个模型"));
                v_vram.set_markup(&format!(
                    "<span size='x-large' weight='bold'>{}</span>",
                    ollama::human_size(vram)
                ));
                s_vram.set_label("已驻留模型合计");

                // 已加载·运行时列表（重建行）
                while let Some(row) = loaded_list.row_at_index(0) {
                    loaded_list.remove(&row);
                }
                if loaded.is_empty() {
                    let r = ListBoxRow::new();
                    r.set_selectable(false);
                    let hint = Label::builder()
                        .label("暂无模型驻留显存 · 首次调用将自动预热（warming）")
                        .css_classes(vec!["dim-label"])
                        .halign(Align::Start)
                        .build();
                    r.set_child(Some(&hint));
                    loaded_list.append(&r);
                } else {
                    for m in &loaded {
                        let row = ListBoxRow::new();
                        row.set_selectable(false);
                        let hb = gtk4::Box::builder()
                            .orientation(Orientation::Horizontal)
                            .spacing(10)
                            .margin_top(8)
                            .margin_bottom(8)
                            .margin_start(10)
                            .margin_end(10)
                            .build();
                        let left = gtk4::Box::builder().orientation(Orientation::Vertical).spacing(2).hexpand(true).build();
                        let name = Label::builder().halign(Align::Start).build();
                        name.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&m.name)));
                        let sub = Label::builder()
                            .css_classes(vec!["dim-label"])
                            .halign(Align::Start)
                            .build();
                        let ka = ollama::models::keep_alive_remaining(&m.expires_at);
                        sub.set_label(&format!("显存 {} · 驻留 {}", m.size_vram_human, ka));
                        left.append(&name);
                        left.append(&sub);
                        hb.append(&left);

                        let client = ollama::client();
                        let nm = m.name.clone();
                        let pin_btn = Button::builder().label("常驻").css_classes(vec!["flat"]).build();
                        {
                            let client = client.clone();
                            let nm = nm.clone();
                            pin_btn.connect_clicked(move |_| {
                                let client = client.clone();
                                let nm = nm.clone();
                                crate::rt::spawn(
                                    async move { ollama::models::pin_model(&client, &nm).await.map_err(|e| e.to_string()) },
                                    move |res: Result<(), String>| {
                                        if let Err(e) = res {
                                            eprintln!("[overview] 常驻失败: {e}");
                                        }
                                    },
                                );
                            });
                        }
                        let client = client.clone();
                        let nm2 = m.name.clone();
                        let win2 = win.clone();
                        let unload_btn = Button::builder().label("卸载").css_classes(vec!["flat"]).build();
                        unload_btn.connect_clicked(move |_| {
                            let client = client.clone();
                            let nm = nm2.clone();
                            let win = win2.clone();
                            crate::rt::spawn(
                                async move { ollama::models::stop_model(&client, &nm).await.map_err(|e| e.to_string()) },
                                move |res: Result<(), String>| {
                                    if let Err(e) = res {
                                        crate::ui::dialogs::show_err(&win, &e);
                                    }
                                },
                            );
                        });
                        hb.append(&pin_btn);
                        hb.append(&unload_btn);
                        row.set_child(Some(&hb));
                        loaded_list.append(&row);
                    }
                }
                // v1.0.16：「最近会话」已移除，轮询只刷新状态卡与已加载列表
            },
        );
    };
    refresh();
    {
        let r = Rc::new(refresh);
        let r2 = Rc::clone(&r);
        glib::timeout_add_local(std::time::Duration::from_secs(3), move || {
            r2();
            glib::ControlFlow::Continue
        });
    }

    root
}

