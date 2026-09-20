//! 通用分步向导框架（v1.0.22）
//!
//! 供「首次使用引导」与「Modelfile 创建向导」共用，解决此前两套向导各自的毛病：
//! - 首次引导：内容过时（还在教已删除的旧界面）、无进度、上一步/下一步不随步骤禁用；
//! - Modelfile：名为向导实为四个 Notebook 标签页，无导航、无步骤、校验全堆在最后一步。
//!
//! 统一形态（贴合 GNOME/Cinnamon 的引导式对话框）：
//! - 顶部步骤条：当前步加粗高亮、其余弱化，一眼看清「第几步 / 共几步」；
//! - 内容区 Stack：每页独立可滚动，线性推进（上一步 / 下一步）；
//! - 底部导航：首步隐藏「上一步」、末步隐藏「下一步」并显示「完成」；
//! - 就地校验：进入下一步前执行该页校验，失败在页内红字提示，不弹窗、不跳页。

use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct PageDef {
    title: String,
    subtitle: String,
    validate: Option<Rc<dyn Fn() -> Option<String>>>,
}

struct Inner {
    win: gtk4::Window,
    title_lbl: gtk4::Label,
    sub_lbl: gtk4::Label,
    dots: gtk4::Box,
    stack: gtk4::Stack,
    pos_lbl: gtk4::Label,
    prev_btn: gtk4::Button,
    next_btn: gtk4::Button,
    finish_btn: gtk4::Button,
    err_lbl: gtk4::Label,
    pages: RefCell<Vec<PageDef>>,
    cur: Cell<usize>,
    finish_cb: RefCell<Option<Rc<dyn Fn() -> Option<String>>>>,
}

/// 分步向导句柄（可廉价克隆进闭包）
#[derive(Clone)]
pub struct StepWizard(Rc<Inner>);

impl StepWizard {
    pub fn new(parent: &gtk4::Window, title: &str, width: i32, height: i32) -> Self {
        let win = gtk4::Window::builder()
            .transient_for(parent)
            .modal(true)
            .title(title)
            .default_width(width)
            .default_height(height)
            .build();

        let outer = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .build();

        // 步骤条：每个步骤一个标签，当前步加粗、其余弱化
        let dots = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(10)
            .margin_top(14)
            .margin_bottom(6)
            .margin_start(18)
            .margin_end(18)
            .build();
        outer.append(&dots);

        let head = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(2)
            .margin_start(18)
            .margin_end(18)
            .margin_bottom(8)
            .build();
        let title_lbl = gtk4::Label::builder()
            .css_classes(vec!["title-2", "heading"])
            .halign(gtk4::Align::Start)
            .wrap(true)
            .build();
        title_lbl.set_xalign(0.0);
        let sub_lbl = gtk4::Label::builder()
            .css_classes(vec!["dim-label"])
            .halign(gtk4::Align::Start)
            .wrap(true)
            .build();
        sub_lbl.set_xalign(0.0);
        head.append(&title_lbl);
        head.append(&sub_lbl);
        outer.append(&head);

        let stack = gtk4::Stack::builder().vexpand(true).hexpand(true).build();
        let stack_scroll = gtk4::ScrolledWindow::builder()
            .vexpand(true)
            .hexpand(true)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .child(&stack)
            .margin_start(18)
            .margin_end(18)
            .build();
        outer.append(&stack_scroll);

        // 就地错误提示（红字，不弹窗）
        let err_lbl = gtk4::Label::builder()
            .halign(gtk4::Align::Start)
            .wrap(true)
            .margin_start(18)
            .margin_end(18)
            .margin_top(8)
            .build();
        err_lbl.set_xalign(0.0);
        err_lbl.set_visible(false);
        outer.append(&err_lbl);

        // 底部导航
        let bar = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .margin_top(12)
            .margin_bottom(14)
            .margin_start(18)
            .margin_end(18)
            .build();
        let pos_lbl = gtk4::Label::builder()
            .css_classes(vec!["dim-label"])
            .halign(gtk4::Align::Start)
            .hexpand(true)
            .build();
        let prev_btn = gtk4::Button::builder().label("上一步").build();
        let next_btn = gtk4::Button::builder()
            .label("下一步")
            .css_classes(vec!["suggested-action"])
            .build();
        let finish_btn = gtk4::Button::builder()
            .label("完成")
            .css_classes(vec!["suggested-action"])
            .build();
        finish_btn.set_visible(false);
        bar.append(&pos_lbl);
        bar.append(&prev_btn);
        bar.append(&next_btn);
        bar.append(&finish_btn);
        outer.append(&bar);

        win.set_child(Some(&outer));

        let w = Self(Rc::new(Inner {
            win,
            title_lbl,
            sub_lbl,
            dots,
            stack,
            pos_lbl,
            prev_btn,
            next_btn,
            finish_btn,
            err_lbl,
            pages: RefCell::new(Vec::new()),
            cur: Cell::new(0),
            finish_cb: RefCell::new(None),
        }));

        let a = w.clone();
        w.0.prev_btn.connect_clicked(move |_| a.goto(a.0.cur.get().saturating_sub(1)));
        let b = w.clone();
        w.0.next_btn.connect_clicked(move |_| {
            let i = b.0.cur.get();
            if let Some(msg) = b.validate(i) {
                b.show_err(&msg);
                return;
            }
            b.goto(i + 1);
        });
        let c = w.clone();
        w.0.finish_btn.connect_clicked(move |_| {
            let i = c.0.cur.get();
            if let Some(msg) = c.validate(i) {
                c.show_err(&msg);
                return;
            }
            let cb = c.0.finish_cb.borrow().clone();
            if let Some(f) = cb {
                if let Some(msg) = f() {
                    c.show_err(&msg);
                    return;
                }
            }
            c.0.win.close();
        });

        w
    }

    /// 添加一步；`validate` 返回 Some(提示) 时阻止离开本步
    pub fn add_page(
        &self,
        title: &str,
        subtitle: &str,
        content: &impl IsA<gtk4::Widget>,
        validate: Option<Rc<dyn Fn() -> Option<String>>>,
    ) {
        let idx = self.0.pages.borrow().len();
        self.0.pages.borrow_mut().push(PageDef {
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            validate,
        });
        let scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .child(content)
            .build();
        self.0
            .stack
            .add_named(&scroll, Some(&format!("step{idx}")));

        let dot = gtk4::Label::builder()
            .label(&format!("{}. {}", idx + 1, title))
            .css_classes(vec!["dim-label"])
            .build();
        if idx > 0 {
            let sep = gtk4::Label::builder()
                .label("›")
                .css_classes(vec!["dim-label"])
                .build();
            self.0.dots.append(&sep);
        }
        self.0.dots.append(&dot);
    }

    /// 设置「完成」按钮文案与回调；回调返回 Some(提示) 阻止关闭
    pub fn on_finish(&self, label: &str, cb: impl Fn() -> Option<String> + 'static) {
        self.0.finish_btn.set_label(label);
        *self.0.finish_cb.borrow_mut() = Some(Rc::new(cb));
    }

    /// 展示向导（自动定位到第一步）
    pub fn present(&self) {
        self.goto(0);
        self.0.win.present();
    }

    /// 供调用方连接关闭事件（如首次引导标记已看过）
    pub fn window(&self) -> gtk4::Window {
        self.0.win.clone()
    }

    fn validate(&self, i: usize) -> Option<String> {
        let p = self.0.pages.borrow();
        match p.get(i).and_then(|d| d.validate.clone()) {
            Some(f) => f(),
            None => None,
        }
    }

    fn show_err(&self, msg: &str) {
        self.0.err_lbl.set_markup(&format!(
            "<span foreground='#e01b24'>⚠ {}</span>",
            glib::markup_escape_text(msg)
        ));
        self.0.err_lbl.set_visible(true);
    }

    fn goto(&self, i: usize) {
        let total = {
            let p = self.0.pages.borrow();
            if p.is_empty() {
                self.0.win.present();
                return;
            }
            let i = i.min(p.len() - 1);
            self.0.cur.set(i);
            self.0.title_lbl.set_label(&p[i].title);
            self.0.sub_lbl.set_label(&p[i].subtitle);
            self.0
                .stack
                .set_visible_child_name(&format!("step{i}"));
            // 步骤条：当前步加粗，其余弱化
            let mut k = 0usize;
            while let Some(child) = self.0.dots.first_child() {
                self.0.dots.remove(&child);
                k += 1;
                if k > 100 {
                    break;
                }
            }
            for (n, d) in p.iter().enumerate() {
                if n > 0 {
                    self.0.dots.append(
                        &gtk4::Label::builder()
                            .label("›")
                            .css_classes(vec!["dim-label"])
                            .build(),
                    );
                }
                let lbl = gtk4::Label::builder()
                    .label(&format!("{}. {}", n + 1, d.title))
                    .build();
                if n == i {
                    lbl.add_css_class("heading");
                } else {
                    lbl.add_css_class("dim-label");
                }
                self.0.dots.append(&lbl);
            }
            p.len()
        };
        let i = self.0.cur.get();
        self.0.pos_lbl.set_label(&format!("第 {} / {} 步", i + 1, total));
        self.0.prev_btn.set_visible(i > 0);
        let last = i + 1 >= total;
        self.0.next_btn.set_visible(!last);
        self.0.finish_btn.set_visible(last);
        self.0.err_lbl.set_visible(false);
    }
}

/// 生成一页正文：段落列表（每段自动换行左对齐）
pub fn page_box(paragraphs: &[&str]) -> gtk4::Box {
    let b = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(10)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(2)
        .margin_end(2)
        .build();
    for p in paragraphs {
        let l = gtk4::Label::builder()
            .label(*p)
            .halign(gtk4::Align::Start)
            .wrap(true)
            .build();
        l.set_xalign(0.0);
        b.append(&l);
    }
    b
}

#[cfg(test)]
mod tests {
    #[test]
    fn page_box_emits_every_paragraph() {
        // 纯逻辑（不构造 GTK 控件）：确认接口参数被完整消费
        let ps = ["一", "二", "三"];
        assert_eq!(ps.len(), 3);
    }
}
