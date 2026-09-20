//! 云库索引编辑器（v3.3.0）。
//!
//! 产品模型：**内置种子为底 + 用户叠加**。
//! - 内置条目随应用升级自动获得，**不可删除**，被改过时可「恢复默认」。
//! - 用户新增条目可改可删。
//! - 所有改动即时写入 `~/.config/offideskollama/catalog_user.json`（叠加层），
//!   不触碰内置种子本体。
//!
//! 交互遵循 GNOME HIG「可编辑列表」规范：
//! - 列表尾部提供「添加模型」按钮；
//! - 每行尾最多两个控件（编辑 + 删除/恢复），图标用 symbolic；
//! - 点击行本身等同「编辑」。
//!
//! 视觉沿用本机 Mint-Y 主题确实定义的样式类：`.rich-list`（富内容行）、
//! `.toolbar`（工具条）、`.heading` `.caption` `.dim-label`（排版）。

use crate::ollama::catalog::{Capability, Catalog, CatalogModel};
use crate::ui::dialogs;
use gtk4::prelude::*;
use gtk4::{
    Align, Box, Button, CheckButton, Entry, Image, Label, ListBox, ListBoxRow, Orientation,
    ScrolledWindow, SearchEntry, SelectionMode, SpinButton, Switch, Window,
};
use std::cell::RefCell;
use std::rc::Rc;

/// 编辑器状态。行/按钮的回调一律持 `Weak`，避免 Rc 环导致窗口无法释放。
struct EditorState {
    /// 编辑器窗口，作为各类子对话框的父级
    win: Window,
    catalog: RefCell<Catalog>,
    /// 当前搜索词（重建列表时复用）
    query: RefCell<String>,
    /// 当前列表视图对应的 id 顺序（供「点击行」定位模型）
    visible: RefCell<Vec<String>>,
    list: ListBox,
    status: Label,
}

/// 打开索引编辑器
pub fn open_editor(parent: &gtk4::Window) {
    let win = Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("云库索引")
        .default_width(780)
        .default_height(640)
        .build();

    // 顶部工具条
    let toolbar = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .css_classes(vec!["toolbar"])
        .margin_top(10)
        .margin_bottom(4)
        .margin_start(12)
        .margin_end(12)
        .build();

    let search = SearchEntry::builder()
        .placeholder_text("搜索索引（名称 / 显示名 / 中文别名）")
        .hexpand(true)
        .build();

    let reset_btn = Button::builder().label("恢复默认").css_classes(vec!["flat"]).build();
    let import_btn = Button::builder().label("导入").css_classes(vec!["flat"]).build();
    let export_btn = Button::builder().label("导出").css_classes(vec!["flat"]).build();
    toolbar.append(&search);
    toolbar.append(&reset_btn);
    toolbar.append(&import_btn);
    toolbar.append(&export_btn);

    // 说明行（诚实交代数据落在哪里、什么不可改）
    let hint = Label::builder()
        .label("内置条目随版本升级自动更新，不可删除（改过可单项恢复）；你新增的条目可自由修改删除。改动即时保存到 ~/.config/offideskollama/catalog_user.json。")
        .halign(Align::Start)
        .wrap(true)
        .css_classes(vec!["caption", "dim-label"])
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(6)
        .build();

    // 列表（.rich-list：主题为富内容行提供 8×12 内边距与 32px 最小高）
    let list = ListBox::builder()
        .selection_mode(SelectionMode::None)
        .css_classes(vec!["rich-list"])
        .build();

    let scroll = ScrolledWindow::builder()
        .child(&list)
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();

    // 底部：状态 + 添加 + 关闭
    let bottom = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .css_classes(vec!["toolbar"])
        .margin_top(6)
        .margin_bottom(10)
        .margin_start(12)
        .margin_end(12)
        .build();
    let add_btn = Button::builder()
        .label("添加模型")
        .icon_name("list-add-symbolic")
        .build();
    let status = Label::builder()
        .halign(Align::End)
        .hexpand(true)
        .css_classes(vec!["caption", "dim-label"])
        .build();
    let close_btn = Button::builder().label("关闭").css_classes(vec!["suggested-action"]).build();
    bottom.append(&add_btn);
    bottom.append(&status);
    bottom.append(&close_btn);

    let outer = Box::builder().orientation(Orientation::Vertical).build();
    outer.append(&toolbar);
    outer.append(&hint);
    outer.append(&scroll);
    outer.append(&bottom);
    win.set_child(Some(&outer));

    let st = Rc::new(EditorState {
        win: win.clone(),
        catalog: RefCell::new(Catalog::load()),
        query: RefCell::new(String::new()),
        visible: RefCell::new(Vec::new()),
        list: list.clone(),
        status: status.clone(),
    });

    // ---- 搜索 ----
    {
        let st_w = Rc::downgrade(&st);
        search.connect_search_changed(move |e| {
            if let Some(st) = st_w.upgrade() {
                *st.query.borrow_mut() = e.text().to_string();
                refresh(&st);
            }
        });
    }

    // ---- 点击行 = 编辑 ----
    {
        let st_w = Rc::downgrade(&st);
        list.connect_row_activated(move |_, row| {
            if let Some(st) = st_w.upgrade() {
                edit_row(&st, row.index());
            }
        });
    }

    // ---- 添加 ----
    {
        let st_w = Rc::downgrade(&st);
        add_btn.connect_clicked(move |_| {
            if let Some(st) = st_w.upgrade() {
                let win = st.win.clone();
                open_form(&win, "添加模型", None, {
                    let st_w2 = Rc::downgrade(&st);
                    move |m| {
                        if let Some(st) = st_w2.upgrade() {
                            st.catalog.borrow_mut().add(m);
                            persist_and_refresh(&st);
                        }
                    }
                });
            }
        });
    }

    // ---- 恢复默认 ----
    {
        let st_w = Rc::downgrade(&st);
        reset_btn.connect_clicked(move |_| {
            let Some(st) = st_w.upgrade() else { return };
            let (added, modified) = st.catalog.borrow().user_summary();
            if added == 0 && modified == 0 {
                dialogs::info(&st.win, "恢复默认", "当前索引与内置默认一致，没有可恢复的自定义内容。");
                return;
            }
            let st_w2 = Rc::downgrade(&st);
            dialogs::confirm(
                &st.win,
                "恢复默认索引",
                &format!(
                    "将丢弃你的全部自定义：新增 {added} 条、修改 {modified} 条，回到内置默认清单。\n此操作不可撤销。"
                ),
                "恢复默认",
                move || {
                    let Some(st) = st_w2.upgrade() else { return };
                    if !Catalog::reset_user() {
                        dialogs::error(
                            &st.win,
                            "恢复失败",
                            "无法删除叠加文件，请检查 ~/.config/offideskollama 的写入权限。",
                        );
                        return;
                    }
                    *st.catalog.borrow_mut() = Catalog::load();
                    *st.query.borrow_mut() = String::new();
                    refresh(&st);
                },
            );
        });
    }

    // ---- 导入 ----
    {
        let st_w = Rc::downgrade(&st);
        import_btn.connect_clicked(move |_| {
            if let Some(st) = st_w.upgrade() {
                do_import(&st);
            }
        });
    }

    // ---- 导出 ----
    {
        let st_w = Rc::downgrade(&st);
        export_btn.connect_clicked(move |_| {
            if let Some(st) = st_w.upgrade() {
                do_export(&st);
            }
        });
    }

    // ---- 关闭 ----
    {
        let w = win.clone();
        close_btn.connect_clicked(move |_| w.close());
    }

    refresh(&st);
    win.present();
}

/// 依当前搜索词重建列表
fn refresh(st: &Rc<EditorState>) {
    while let Some(c) = st.list.first_child() {
        st.list.remove(&c);
    }
    let query = st.query.borrow().clone();
    let models: Vec<CatalogModel> = {
        let cat = st.catalog.borrow();
        if query.trim().is_empty() {
            cat.all().to_vec()
        } else {
            cat.search(&query).into_iter().cloned().collect()
        }
    };
    *st.visible.borrow_mut() = models.iter().map(|m| m.id.clone()).collect();
    for m in &models {
        st.list.append(&row_widget(st, m));
    }
    update_status(st);
}

/// 单行：图标 + 标题/副标题 + 行尾控件（编辑 + 删除/恢复）
fn row_widget(st: &Rc<EditorState>, m: &CatalogModel) -> ListBoxRow {
    let row = ListBoxRow::new();
    let hb = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .build();

    let builtin = Catalog::is_builtin(&m.id);
    let modified = st.catalog.borrow().is_modified(&m.id);

    hb.append(
        &Image::builder()
            .icon_name(if builtin { "package-x-generic-symbolic" } else { "starred-symbolic" })
            .valign(Align::Center)
            .build(),
    );

    let col = Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .valign(Align::Center)
        .build();

    let title = Label::builder()
        .label(&m.display)
        .halign(Align::Start)
        .css_classes(vec!["heading"])
        .build();

    let origin = if builtin { "内置" } else { "自定义" };
    let caps = if m.caps.is_empty() { "未标能力".to_string() } else { m.caps_summary() };
    let sub = Label::builder()
        .label(format!(
            "{}:{} · 约 {:.2} GB · {caps} · {origin}{}",
            m.name,
            m.ref_tag,
            m.ref_size_gb,
            if modified { "（已改）" } else { "" }
        ))
        .halign(Align::Start)
        .css_classes(vec!["caption", "dim-label"])
        .build();

    col.append(&title);
    col.append(&sub);
    hb.append(&col);

    // 编辑
    let edit = Button::builder()
        .icon_name("document-edit-symbolic")
        .tooltip_text("编辑")
        .css_classes(vec!["flat"])
        .valign(Align::Center)
        .build();
    {
        let st_w = Rc::downgrade(st);
        let m = m.clone();
        edit.connect_clicked(move |_| {
            if let Some(st) = st_w.upgrade() {
                open_edit_form(&st, &m);
            }
        });
    }
    hb.append(&edit);

    // 行尾第二个控件：内置→恢复（仅被改过时）；自定义→删除
    if builtin {
        if modified {
            let restore = Button::builder()
                .icon_name("edit-undo-symbolic")
                .tooltip_text("恢复为内置默认")
                .css_classes(vec!["flat"])
                .valign(Align::Center)
                .build();
            {
                let st_w = Rc::downgrade(st);
                let id = m.id.clone();
                restore.connect_clicked(move |_| {
                    if let Some(st) = st_w.upgrade() {
                        st.catalog.borrow_mut().reset_entry(&id);
                        persist_and_refresh(&st);
                    }
                });
            }
            hb.append(&restore);
        }
    } else {
        let del = Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("删除此条目")
            .css_classes(vec!["flat"])
            .valign(Align::Center)
            .build();
        {
            let st_w = Rc::downgrade(st);
            let id = m.id.clone();
            let display = m.display.clone();
            del.connect_clicked(move |_| {
                let Some(st) = st_w.upgrade() else { return };
                let st_w2 = Rc::downgrade(&st);
                let id = id.clone();
                dialogs::confirm(
                    &st.win,
                    "删除条目",
                    &format!("确定从索引中删除「{display}」？此操作不可撤销。"),
                    "删除",
                    move || {
                        if let Some(st) = st_w2.upgrade() {
                            st.catalog.borrow_mut().remove(&id);
                            persist_and_refresh(&st);
                        }
                    },
                );
            });
        }
        hb.append(&del);
    }

    row.set_child(Some(&hb));
    row.set_tooltip_text(Some("点击行可编辑"));
    row
}

/// 点击行 → 按当前视图顺序定位模型并编辑
fn edit_row(st: &Rc<EditorState>, index: i32) {
    if index < 0 {
        return;
    }
    let id = st.visible.borrow().get(index as usize).cloned();
    let Some(id) = id else { return };
    let m = st.catalog.borrow().all().iter().find(|m| m.id == id).cloned();
    if let Some(m) = m {
        open_edit_form(st, &m);
    }
}

fn open_edit_form(st: &Rc<EditorState>, m: &CatalogModel) {
    let win = st.win.clone();
    let initial = m.clone();
    let st_w = Rc::downgrade(st);
    open_form(&win, "编辑模型", Some(&initial), move |new| {
        if let Some(st) = st_w.upgrade() {
            let id = new.id.clone();
            st.catalog.borrow_mut().update(&id, new);
            persist_and_refresh(&st);
        }
    });
}

/// 保存叠加层并刷新列表
fn persist_and_refresh(st: &Rc<EditorState>) {
    if !st.catalog.borrow().save() {
        dialogs::error(
            &st.win,
            "保存失败",
            "无法写入 ~/.config/offideskollama/catalog_user.json（磁盘已满或权限不足）。本次修改未被保存。",
        );
        return;
    }
    refresh(st);
}

fn update_status(st: &Rc<EditorState>) {
    let cat = st.catalog.borrow();
    let total = cat.len();
    let builtin = cat.all().iter().filter(|m| Catalog::is_builtin(&m.id)).count();
    let (added, modified) = cat.user_summary();
    st.status.set_label(&format!(
        "共 {total} 条 · 内置 {builtin} · 自定义 {added} · 已改 {modified}"
    ));
}

// ============================================================
// 行编辑表单
// ============================================================

/// 模型编辑表单。`initial` 为 None 表示新增。
fn open_form(
    parent: &Window,
    title: &str,
    initial: Option<&CatalogModel>,
    on_submit: impl Fn(CatalogModel) + 'static,
) {
    let win = Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(title)
        .default_width(540)
        .build();

    let form = Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(16)
        .margin_end(16)
        .build();

    let field = |label: &str| -> Label {
        Label::builder().label(label).halign(Align::Start).margin_top(8).build()
    };

    form.append(&field("ollama 模型名（如 qwen2.5，不带标签）"));
    let name_e = Entry::builder()
        .text(initial.map(|m| m.name.clone()).unwrap_or_default())
        .placeholder_text("qwen2.5")
        .build();
    form.append(&name_e);

    form.append(&field("显示名（列表里展示的名字）"));
    let display_e = Entry::builder()
        .text(initial.map(|m| m.display.clone()).unwrap_or_default())
        .build();
    form.append(&display_e);

    form.append(&field("一句话简介"));
    let desc_e = Entry::builder()
        .text(initial.map(|m| m.desc.clone()).unwrap_or_default())
        .build();
    form.append(&desc_e);

    form.append(&field("能力标签（可多选）"));
    let cap_row = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .build();
    let mut cap_btns: Vec<(Capability, CheckButton)> = Vec::new();
    for cap in Capability::ALL {
        let cb = CheckButton::builder().label(cap.label()).build();
        if let Some(m) = initial {
            cb.set_active(m.caps.contains(&cap));
        } else if cap == Capability::Chat {
            cb.set_active(true);
        }
        cap_row.append(&cb);
        cap_btns.push((cap, cb));
    }
    form.append(&cap_row);

    let zh_row = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(8)
        .build();
    let zh_lbl = Label::builder().label("中文优化").halign(Align::Start).hexpand(true).build();
    let zh_sw = Switch::builder().valign(Align::Center).build();
    zh_sw.set_active(initial.map(|m| m.chinese).unwrap_or(false));
    zh_row.append(&zh_lbl);
    zh_row.append(&zh_sw);
    form.append(&zh_row);

    let num_row = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(8)
        .build();
    let size_col = Box::builder().orientation(Orientation::Vertical).spacing(2).hexpand(true).build();
    size_col.append(&Label::builder().label("参考体积（GB，仅用于适配估算）").halign(Align::Start).build());
    let adj = gtk4::Adjustment::new(
        initial.map(|m| m.ref_size_gb).unwrap_or(4.0),
        0.0,
        4000.0,
        0.1,
        1.0,
        0.0,
    );
    let size_sp = SpinButton::builder().adjustment(&adj).digits(2).numeric(true).build();
    size_col.append(&size_sp);
    let tag_col = Box::builder().orientation(Orientation::Vertical).spacing(2).build();
    tag_col.append(&Label::builder().label("推荐标签").halign(Align::Start).build());
    let tag_e = Entry::builder()
        .text(initial.map(|m| m.ref_tag.clone()).unwrap_or_else(|| "latest".into()))
        .width_request(140)
        .build();
    tag_col.append(&tag_e);
    num_row.append(&size_col);
    num_row.append(&tag_col);
    form.append(&num_row);

    let bar = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(Align::End)
        .margin_top(14)
        .build();
    let cancel = Button::builder().label("取消").build();
    let save = Button::builder().label("保存").css_classes(vec!["suggested-action"]).build();
    bar.append(&cancel);
    bar.append(&save);
    form.append(&bar);

    let scroll = ScrolledWindow::builder()
        .child(&form)
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();
    win.set_child(Some(&scroll));

    {
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
    }

    let on_submit = Rc::new(on_submit);
    {
        let win_c = win.clone();
        let name_e = name_e.clone();
        let display_e = display_e.clone();
        let desc_e = desc_e.clone();
        let tag_e = tag_e.clone();
        let size_sp = size_sp.clone();
        let zh_sw = zh_sw.clone();
        let cap_btns = cap_btns.clone();
        let id = initial.map(|m| m.id.clone()).unwrap_or_default();
        save.connect_clicked(move |_| {
            let name = name_e.text().trim().to_string();
            if name.is_empty() {
                dialogs::error(&win_c, "无法保存", "「ollama 模型名」不能为空。");
                return;
            }
            let mut display = display_e.text().trim().to_string();
            if display.is_empty() {
                display = name.clone();
            }
            let caps: Vec<Capability> = cap_btns
                .iter()
                .filter(|(_, cb)| cb.is_active())
                .map(|(c, _)| *c)
                .collect();
            let mut tag = tag_e.text().trim().to_string();
            if tag.is_empty() {
                tag = "latest".into();
            }
            let m = CatalogModel {
                id: id.clone(),
                name,
                display,
                desc: desc_e.text().trim().to_string(),
                caps,
                chinese: zh_sw.is_active(),
                ref_size_gb: size_sp.value(),
                ref_tag: tag,
            };
            on_submit(m);
            win_c.close();
        });
    }

    win.present();
}

// ============================================================
// 导入 / 导出
// ============================================================

fn do_export(st: &Rc<EditorState>) {
    let (added, modified) = st.catalog.borrow().user_summary();
    if added == 0 && modified == 0 {
        dialogs::info(
            &st.win,
            "无可导出内容",
            "当前索引与内置默认一致，没有自定义内容可导出。\n（内置清单随应用升级自动更新，无需导出。）",
        );
        return;
    }
    let json = match st.catalog.borrow().to_json() {
        Ok(j) => j,
        Err(e) => {
            dialogs::error(&st.win, "导出失败", &e);
            return;
        }
    };
    let d = gtk4::FileChooserNative::new(
        Some("导出云库索引"),
        Some(&st.win),
        gtk4::FileChooserAction::Save,
        Some("保存"),
        Some("取消"),
    );
    d.set_current_name("catalog_user.json");
    // 原生对话框在 show 后必须持有引用，否则 GTK 会在可见期间销毁它导致段错误。
    let d_keep = d.clone();
    let parent = st.win.clone();
    d.connect_response(move |chooser, resp| {
        let _alive = &d_keep;
        if resp != gtk4::ResponseType::Accept {
            return;
        }
        if let Some(path) = chooser.file().and_then(|f| f.path()) {
            if let Err(e) = std::fs::write(&path, &json) {
                dialogs::error(&parent, "导出失败", &e.to_string());
            }
        }
    });
    d.show();
}

fn do_import(st: &Rc<EditorState>) {
    let d = gtk4::FileChooserNative::new(
        Some("导入云库索引"),
        Some(&st.win),
        gtk4::FileChooserAction::Open,
        Some("打开"),
        Some("取消"),
    );
    let d_keep = d.clone();
    let st_w = Rc::downgrade(st);
    d.connect_response(move |chooser, resp| {
        let _alive = &d_keep;
        if resp != gtk4::ResponseType::Accept {
            return;
        }
        let Some(st) = st_w.upgrade() else { return };
        let Some(path) = chooser.file().and_then(|f| f.path()) else { return };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                dialogs::error(&st.win, "读取失败", &e.to_string());
                return;
            }
        };
        let result = st.catalog.borrow_mut().import_json(&text);
        match result {
            Ok(n) => {
                persist_and_refresh(&st);
                dialogs::info(&st.win, "导入完成", &format!("已合并 {n} 条索引条目。"));
            }
            Err(e) => dialogs::error(
                &st.win,
                "导入失败",
                &format!(
                    "{e}\n\n支持两种格式：\n· 叠加层对象 {{\"added\":[…],\"overrides\":[…]}}\n· 或裸模型数组 […]"
                ),
            ),
        }
    });
    d.show();
}
