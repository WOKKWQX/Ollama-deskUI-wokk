//! 模型云库：主窗口「云库」页。
//!
//! 定位（v3.5.0 重写）：云库 = **推荐层 + 检索层**两层，而不是一张冻结的精选列表。
//! - 推荐层（离线可用）：内置精选清单 + 用户叠加（`catalog::Catalog`），按硬件标适配。
//! - 检索层（联网）：本页内直接检索 Ollama 官方库并按标签拉取；本地零命中时
//!   给出明确空状态与「去官方库搜索」的出口，不再让用户面对一片空白。
//!
//! v3.5.0 修复的缺陷（逐条来自源码走查）：
//! 1. 「已安装」按基名匹配导致误报（装了 0.5b 也把 7b 卡片标已安装）→ 改为按
//!    完整标签精确判定，其它标签另行列示。
//! 2. 下载完成后卡片状态不回填 → `pull_flow_with` 完成回调驱动本页刷新。
//! 3. 拉取进度全屏模态且不可取消 → 进度框非模态 + 取消按钮（取消即断开下载流）。
//! 4. 安装无确认、不查磁盘 → 安装前确认框（含体积与目标分区剩余空间）。
//! 5. 对比勾选状态一重建就丢、超过 3 个静默失败 → 状态持久化 + 明确上限提示。
//! 6. 适配徽章硬编码色值且橙色对比度不达 WCAG AA → 改 symbolic 图标 + 主题类。
//! 7. 「已安装」分类漏掉非精选库模型 → 直接以本机真实条目成卡（含真实体积）。
//! 8. 详情页离线时无安装入口 → 参考估算行补安装按钮。
//! 9. 无排序、搜索无防抖、页脚与硬件行重复 → 见各自实现。
//!
//! 诚实边界（沿用）：体积一律标注「参考」；官方库标签体积取不到时显示「未知」，
//! 不伪造；不显示任何伪造的下载量/热度。

use crate::ollama::catalog::{score_model, Capability, Catalog, CatalogModel, Category};
use crate::ollama::hardware::{
    detect_hardware, fit_for, load_hardware, save_hardware, Fit, HardwareProfile,
};
use crate::ollama::{human_size, library, models, ModelInfo};
use crate::rt;
use crate::ui::dialogs;
use crate::ui::sidebar::{self, Sidebar};
use gtk4::prelude::*;
use gtk4::{
    Button, CheckButton, DropDown, FlowBox, Grid, Image, Label, Orientation, ScrolledWindow,
    SearchEntry, Stack, ToggleButton, Window,
};
use std::cell::RefCell;
use std::rc::Rc;

/// 任务导向筛选（顶部 chips）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Task {
    Chat,
    Code,
    Vision,
    Embed,
    Reason,
    Weak,
}

impl Task {
    fn label(&self) -> &'static str {
        match self {
            Task::Chat => "聊天",
            Task::Code => "写代码",
            Task::Vision => "读图",
            Task::Embed => "嵌入",
            Task::Reason => "强推理",
            Task::Weak => "老机器",
        }
    }
    fn cap(&self) -> Capability {
        match self {
            Task::Chat => Capability::Chat,
            Task::Code => Capability::Code,
            Task::Vision => Capability::Vision,
            Task::Embed => Capability::Embed,
            Task::Reason => Capability::Reason,
            Task::Weak => Capability::Chat,
        }
    }
}

/// 排序方式。V4.0.0 起「推荐评分」（白盒三维：任务+适配+官方热度）为默认。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortMode {
    Score,
    Default,
    FitFirst,
    SizeAsc,
    SizeDesc,
    Name,
}

impl SortMode {
    fn labels() -> [&'static str; 6] {
        [
            "推荐评分",
            "默认排序",
            "适配优先",
            "体积小→大",
            "体积大→小",
            "按名称",
        ]
    }
    fn from_index(i: u32) -> Self {
        match i {
            1 => SortMode::Default,
            2 => SortMode::FitFirst,
            3 => SortMode::SizeAsc,
            4 => SortMode::SizeDesc,
            5 => SortMode::Name,
            _ => SortMode::Score,
        }
    }
}

/// 卡片承载的内容：精选条目，或本机已装但不在精选库中的真实条目。
#[derive(Clone)]
enum CardItem {
    Catalog(CatalogModel),
    Local(ModelInfo),
}

impl CardItem {
    fn sort_key_size(&self) -> f64 {
        match self {
            CardItem::Catalog(m) => m.ref_size_gb,
            CardItem::Local(m) => m.size as f64 / 1e9,
        }
    }
    fn sort_key_name(&self) -> String {
        match self {
            CardItem::Catalog(m) => m.name.to_lowercase(),
            CardItem::Local(m) => m.name.to_lowercase(),
        }
    }
    fn size_bytes(&self) -> u64 {
        match self {
            CardItem::Catalog(m) => (m.ref_size_gb * 1e9) as u64,
            CardItem::Local(m) => m.size,
        }
    }
}

/// 云库页的可变状态（整体包在 Rc 里，供各处信号回调共享）
struct DiscoverState {
    win: Window,
    sidebar: Rc<Sidebar>,
    on_chat: Rc<dyn Fn(String)>,
    /// 目录（内置种子 + 用户叠加），运行期只读
    catalog: RefCell<Catalog>,
    profile: RefCell<HardwareProfile>,
    /// 本机已装模型（真实条目，含真实体积/量化），用于状态判定与「已安装」视图
    installed: RefCell<Vec<ModelInfo>>,
    category: RefCell<Category>,
    task: RefCell<Option<Task>>,
    sort: RefCell<SortMode>,
    search: RefCell<String>,
    /// 搜索防抖代际计数
    search_gen: RefCell<u64>,
    /// 对比选中（持久化，跨卡片重建保留）
    selected: RefCell<Vec<CatalogModel>>,
    /// 正在下载中的完整模型名（卡片显示「下载中…」并禁用重复点击）
    downloads: RefCell<Vec<String>>,
    toggles: RefCell<Vec<(Task, ToggleButton)>>,
    setting_toggle: RefCell<bool>,
    flow: FlowBox,
    /// 「显示更多」分页按钮（千条规模下首屏只渲染前 60 张卡）
    more_btn: Button,
    footer: Label,
    hw_label: Label,
    compare_btn: RefCell<Button>,
    stack: Stack,
    empty_label: Label,
    official_entry: gtk4::Entry,
    official_status: Label,
    official_list: gtk4::Box,
    /// 分页容量：当前视图最多渲染的卡片数（滚动「显示更多」递增）
    page_cap: RefCell<usize>,
    /// 最近一次筛选结果的完整集合（「显示更多」直接增量重渲染，免重算）
    last_items: RefCell<Vec<CardItem>>,
}

/// 构建「云库」页内容，返回可加入 gtk4::Stack 的控件
pub fn build_discover_page(
    parent: &Window,
    sidebar: &Rc<Sidebar>,
    on_chat: Rc<dyn Fn(String)>,
) -> gtk4::Box {
    let profile = load_hardware().unwrap_or_else(detect_hardware);

    let flow = FlowBox::builder()
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(10)
        .margin_end(10)
        .selection_mode(gtk4::SelectionMode::None)
        .max_children_per_line(2)
        .min_children_per_line(1)
        .column_spacing(12)
        .row_spacing(12)
        .halign(gtk4::Align::Fill)
        .build();

    // 「显示更多」：千条规模分页加载（创建需早于 DiscoverState）
    let more_btn = Button::builder()
        .label("显示更多")
        .css_classes(vec!["flat"])
        .visible(false)
        .halign(gtk4::Align::Center)
        .margin_bottom(20)
        .build();

    let footer = Label::builder()
        .css_classes(vec!["caption", "dim-label"])
        .halign(gtk4::Align::Start)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(8)
        .build();
    let hw_label = Label::builder()
        .css_classes(vec!["caption", "dim-label"])
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();

    let stack = Stack::builder().vexpand(true).build();
    let empty_label = Label::builder()
        .css_classes(vec!["dim-label"])
        .halign(gtk4::Align::Center)
        .justify(gtk4::Justification::Center)
        .wrap(true)
        .build();
    let empty_btn = Button::builder().label("在 Ollama 官方库搜索").build();
    let official_entry = gtk4::Entry::builder()
        .hexpand(true)
        .placeholder_text("模型名，如 llama3.2 / qwen2.5 / gemma3")
        .activates_default(true)
        .build();
    let official_status = Label::builder()
        .css_classes(vec!["caption", "dim-label"])
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    let official_list = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .build();

    let st = Rc::new(DiscoverState {
        win: parent.clone(),
        sidebar: Rc::clone(sidebar),
        on_chat,
        catalog: RefCell::new(Catalog::load()),
        profile: RefCell::new(profile),
        installed: RefCell::new(Vec::new()),
        category: RefCell::new(Category::All),
        task: RefCell::new(None),
        sort: RefCell::new(SortMode::Score),
        search: RefCell::new(String::new()),
        search_gen: RefCell::new(0),
        selected: RefCell::new(Vec::new()),
        downloads: RefCell::new(Vec::new()),
        toggles: RefCell::new(Vec::new()),
        setting_toggle: RefCell::new(false),
        flow: flow.clone(),
        more_btn: more_btn.clone(),
        page_cap: RefCell::new(60),
        last_items: RefCell::new(Vec::new()),
        footer: footer.clone(),
        hw_label: hw_label.clone(),
        compare_btn: RefCell::new(Button::builder().label("对比 (0)").build()),
        stack: stack.clone(),
        empty_label: empty_label.clone(),
        official_entry: official_entry.clone(),
        official_status: official_status.clone(),
        official_list: official_list.clone(),
    });

    // ---- 顶部工具条 ----
    let toolbar = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .css_classes(vec!["toolbar"])
        .margin_top(8)
        .margin_start(10)
        .margin_end(10)
        .build();

    let search = SearchEntry::builder()
        .placeholder_text("搜索模型（支持中文：千问 / 羊驼 / 智谱…）")
        .hexpand(true)
        .build();
    {
        let st = Rc::clone(&st);
        search.connect_search_changed(move |e| {
            *st.search.borrow_mut() = e.text().to_string();
            *st.task.borrow_mut() = None;
            clear_toggles(&st);
            // 防抖 150ms：此前每敲一个键都全量重建 68 张卡片，输入发涩
            let gen = {
                let mut g = st.search_gen.borrow_mut();
                *g += 1;
                *g
            };
            let st_d = Rc::clone(&st);
            glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                if *st_d.search_gen.borrow() == gen {
                    apply_filter(&st_d);
                }
            });
        });
    }
    toolbar.append(&search);

    let actions = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .css_classes(vec!["linked"])
        .valign(gtk4::Align::Center)
        .build();
    let recommend_btn = Button::builder().label("为你选").build();
    {
        let st = Rc::clone(&st);
        recommend_btn.connect_clicked(move |_| show_recommend(&st));
    }
    actions.append(&recommend_btn);
    {
        let st_c = Rc::clone(&st);
        st.compare_btn
            .borrow()
            .connect_clicked(move |_| show_compare(&st_c));
    }
    actions.append(&*st.compare_btn.borrow());
    toolbar.append(&actions);

    // ---- 第二行：分类 + 任务 chips + 排序 + 官方库入口 ----
    let filter_row = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .css_classes(vec!["toolbar"])
        .margin_top(2)
        .margin_start(10)
        .margin_end(10)
        .build();

    let cats: Vec<Category> = vec![
        Category::All,
        Category::Curated,
        Category::Chat,
        Category::Code,
        Category::Reason,
        Category::Vision,
        Category::Embed,
        Category::Chinese,
        Category::Small,
        Category::Installed,
    ];
    let cat_labels: Vec<&str> = cats.iter().map(|c| c.label()).collect();
    let cat_dd = DropDown::from_strings(&cat_labels);
    cat_dd.set_width_request(104);
    {
        let st = Rc::clone(&st);
        let cats = cats.clone();
        cat_dd.connect_selected_notify(move |dd| {
            let idx = dd.selected() as usize;
            if idx < cats.len() {
                *st.category.borrow_mut() = cats[idx];
                *st.task.borrow_mut() = None;
                clear_toggles(&st);
                apply_filter(&st);
            }
        });
    }
    filter_row.append(&cat_dd);

    let chips = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .css_classes(vec!["linked"])
        .halign(gtk4::Align::Start)
        .build();
    for t in [
        Task::Chat,
        Task::Code,
        Task::Vision,
        Task::Embed,
        Task::Reason,
        Task::Weak,
    ] {
        let tb = ToggleButton::builder().label(t.label()).build();
        {
            let st = Rc::clone(&st);
            let task = t;
            tb.connect_toggled(move |btn| {
                if *st.setting_toggle.borrow() {
                    return;
                }
                *st.setting_toggle.borrow_mut() = true;
                if btn.is_active() {
                    set_task(&st, Some(task));
                } else if *st.task.borrow() == Some(task) {
                    set_task(&st, None);
                } else {
                    btn.set_active(false);
                }
                *st.setting_toggle.borrow_mut() = false;
            });
        }
        st.toggles.borrow_mut().push((t, tb.clone()));
        chips.append(&tb);
    }
    filter_row.append(&chips);

    let spacer = gtk4::Box::builder().hexpand(true).build();
    filter_row.append(&spacer);
    let sort_dd = DropDown::from_strings(&SortMode::labels());
    sort_dd.set_width_request(108);
    {
        let st = Rc::clone(&st);
        sort_dd.connect_selected_notify(move |dd| {
            *st.sort.borrow_mut() = SortMode::from_index(dd.selected());
            apply_filter(&st);
        });
    }
    filter_row.append(&sort_dd);

    let official_btn = Button::builder()
        .label("搜官方库")
        .css_classes(vec!["flat"])
        .build();
    {
        let st = Rc::clone(&st);
        let entry = search.clone();
        official_btn.connect_clicked(move |_| {
            let q = entry.text().trim().to_string();
            show_official(&st, &q);
        });
    }
    filter_row.append(&official_btn);

    // ---- 第三行：硬件状态 + 维护动作 ----
    let hw_row = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .css_classes(vec!["toolbar"])
        .margin_top(2)
        .margin_start(10)
        .margin_end(10)
        .build();
    hw_label.set_hexpand(true);
    hw_label.set_valign(gtk4::Align::Center);
    hw_row.append(&hw_label);
    let refresh_btn = Button::builder().label("刷新已装").css_classes(vec!["flat"]).build();
    {
        let st = Rc::clone(&st);
        refresh_btn.connect_clicked(move |_| refresh_installed(&st));
    }
    let hw_btn = Button::builder().label("重测硬件").css_classes(vec!["flat"]).build();
    {
        let st = Rc::clone(&st);
        hw_btn.connect_clicked(move |_| redetect_hardware(&st));
    }
    let manual_btn = Button::builder().label("填显存").css_classes(vec!["flat"]).build();
    {
        let st = Rc::clone(&st);
        manual_btn.connect_clicked(move |_| manual_vram(&st));
    }
    hw_row.append(&refresh_btn);
    hw_row.append(&hw_btn);
    hw_row.append(&manual_btn);

    // ---- Stack 三页：卡片 / 空状态 / 官方库 ----
    // 千条规模分页：首屏 60 张，其余走「显示更多」（按钮已在 flow 旁创建）
    let cards_box = gtk4::Box::builder().orientation(Orientation::Vertical).build();
    cards_box.append(&flow);
    cards_box.append(&more_btn);
    let flow_scroll = ScrolledWindow::builder()
        .child(&cards_box)
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();
    stack.add_named(&flow_scroll, Some("cards"));

    {
        let st = Rc::clone(&st);
        more_btn.connect_clicked(move |_| {
            *st.page_cap.borrow_mut() += 60;
            let items = st.last_items.borrow().clone();
            render_cards(&st, &items);
        });
    }

    let empty_box = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .valign(gtk4::Align::Center)
        .halign(gtk4::Align::Center)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();
    empty_box.append(&empty_label);
    empty_box.append(&empty_btn);
    {
        let st = Rc::clone(&st);
        empty_btn.connect_clicked(move |_| {
            let q = st.search.borrow().clone();
            show_official(&st, &q);
        });
    }
    stack.add_named(&empty_box, Some("empty"));

    let official_box = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(10)
        .margin_end(10)
        .build();
    let back_btn = Button::builder().label("← 返回精选").css_classes(vec!["flat"]).build();
    {
        let st = Rc::clone(&st);
        back_btn.connect_clicked(move |_| {
            *st.search.borrow_mut() = String::new();
            st.official_entry.set_text("");
            apply_filter(&st);
        });
    }
    let official_head = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .build();
    official_head.append(&back_btn);
    official_head.append(
        &Label::builder()
            .label("Ollama 官方库")
            .halign(gtk4::Align::Start)
            .hexpand(true)
            .css_classes(vec!["heading"])
            .build(),
    );
    official_box.append(&official_head);

    let official_row = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .css_classes(vec!["linked"])
        .build();
    official_row.append(&official_entry);
    let query_btn = Button::builder().label("查询").build();
    official_row.append(&query_btn);
    official_box.append(&official_row);
    official_box.append(&official_status);
    {
        let st = Rc::clone(&st);
        let entry = official_entry.clone();
        let q: Rc<dyn Fn()> = Rc::new(move || {
            let text = entry.text().trim().to_string();
            show_official(&st, &text);
        });
        {
            let q1 = Rc::clone(&q);
            query_btn.connect_clicked(move |_| q1());
        }
        {
            let q2 = Rc::clone(&q);
            official_entry.connect_activate(move |_| q2());
        }
    }
    let official_scroll = ScrolledWindow::builder()
        .child(&official_list)
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();
    official_box.append(&official_scroll);
    stack.add_named(&official_box, Some("official"));

    // ---- 组装 ----
    let outer = gtk4::Box::builder().orientation(Orientation::Vertical).build();
    outer.append(&toolbar);
    outer.append(&filter_row);
    outer.append(&hw_row);
    outer.append(&stack);
    outer.append(&footer);

    // 初始填充
    update_hw_label(&st);
    refresh_installed(&st);
    apply_filter(&st);

    // 每次本页被切到前台（Stack 重新 map）时，从磁盘重载索引——
    // 这样在「设置 → 云库索引」里改完，切回云库即可见，无需重启应用。
    {
        let st = Rc::clone(&st);
        outer.connect_map(move |_| {
            *st.catalog.borrow_mut() = Catalog::load();
            refresh_installed(&st);
        });
    }

    outer
}

// ============================================================
// 适配徽章（v3.5.0：去硬编码色值）
// ============================================================

/// 适配徽章：symbolic 图标 + 文字 + 主题语义类。
///
/// 此前用 Pango markup 写死 hex 背景（橙色 `#b26a00` + 白字对比度约 4.2:1，
/// 低于 WCAG 2.1 AA 正文 4.5:1）。现改用主题自身定义的 `.warning` / `.error`
/// 与系统 symbolic 图标：不写死颜色、跟随系统主题，且状态不靠颜色单独表达
/// （图标形状同样承载语义）。
fn fit_badge(fit: Fit) -> gtk4::Box {
    let (icon, cls) = match fit {
        Fit::Smooth => ("emblem-ok-symbolic", None),
        Fit::Tight => ("dialog-warning-symbolic", Some("warning")),
        Fit::CpuOnly => ("dialog-information-symbolic", None),
        Fit::NoFit => ("dialog-error-symbolic", Some("error")),
    };
    let b = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(3)
        .valign(gtk4::Align::Center)
        .build();
    b.append(&Image::from_icon_name(icon));
    let l = Label::builder().label(fit.label()).css_classes(vec!["caption"]).build();
    if let Some(c) = cls {
        l.add_css_class(c);
    }
    b.append(&l);
    b.set_tooltip_text(Some(fit.explain()));
    b
}

// ============================================================
// 已安装状态判定（v3.5.0：精确到标签）
// ============================================================

/// 某模型基名下本机已装的全部标签
fn installed_tags_for(installed: &[ModelInfo], name: &str) -> Vec<String> {
    let prefix = format!("{}:", name);
    let mut out = Vec::new();
    for mi in installed {
        if mi.name == name {
            out.push("latest".to_string());
        } else if let Some(t) = mi.name.strip_prefix(&prefix) {
            out.push(t.to_string());
        }
    }
    out
}

// ============================================================
// 卡片与列表
// ============================================================

/// 单张卡片：以 `.frame` 作为卡片边框（本机主题确有该样式），内层留白
fn card_widget(st: &Rc<DiscoverState>, item: &CardItem) -> gtk4::Box {
    let card = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .css_classes(vec!["frame"])
        .build();
    let box_ = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(12)
        .margin_end(12)
        .build();

    let fit = fit_for(item.size_bytes(), &st.profile.borrow());
    let top = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .build();

    match item {
        CardItem::Catalog(m) => {
            top.append(
                &Label::builder()
                    .label(&m.display)
                    .halign(gtk4::Align::Start)
                    .css_classes(vec!["heading"])
                    .wrap(true)
                    .build(),
            );
            top.append(&fit_badge(fit));

            let tags = installed_tags_for(&st.installed.borrow(), &m.name);
            let ref_installed = tags.iter().any(|t| t == &m.ref_tag);
            let other_tags: Vec<String> = tags
                .iter()
                .filter(|t| *t != &m.ref_tag)
                .cloned()
                .collect();
            if ref_installed {
                let mark = gtk4::Box::builder()
                    .orientation(Orientation::Horizontal)
                    .spacing(2)
                    .valign(gtk4::Align::Center)
                    .build();
                mark.append(&Image::from_icon_name("object-select-symbolic"));
                mark.append(
                    &Label::builder()
                        .label("已安装")
                        .css_classes(vec!["caption"])
                        .build(),
                );
                mark.set_tooltip_text(Some("推荐标签已在本机"));
                top.append(&mark);
            }
            box_.append(&top);

            box_.append(
                &Label::builder()
                    .label(format!(
                        "{}:{} · 约 {:.2} GB（参考）",
                        m.name, m.ref_tag, m.ref_size_gb
                    ))
                    .halign(gtk4::Align::Start)
                    .css_classes(vec!["caption", "dim-label"])
                    .build(),
            );
            if !other_tags.is_empty() {
                box_.append(
                    &Label::builder()
                        .label(format!("本机另有标签：{}", other_tags.join(" / ")))
                        .halign(gtk4::Align::Start)
                        .css_classes(vec!["caption", "dim-label"])
                        .build(),
                );
            }
            box_.append(
                &Label::builder()
                    .label(&m.desc)
                    .halign(gtk4::Align::Start)
                    .css_classes(vec!["dim-label"])
                    .wrap(true)
                    .build(),
            );
            box_.append(
                &Label::builder()
                    .label(m.caps_summary())
                    .halign(gtk4::Align::Start)
                    .css_classes(vec!["caption"])
                    .build(),
            );

            let btn_row = gtk4::Box::builder()
                .orientation(Orientation::Horizontal)
                .spacing(6)
                .margin_top(4)
                .build();
            let full = format!("{}:{}", m.name, m.ref_tag);
            let downloading = st.downloads.borrow().iter().any(|d| d == &full);
            if downloading {
                btn_row.append(&Button::builder().label("下载中…").sensitive(false).build());
            } else if ref_installed {
                let b = Button::builder()
                    .label("对话")
                    .css_classes(vec!["suggested-action"])
                    .build();
                {
                    let st = Rc::clone(st);
                    let name = m.name.clone();
                    b.connect_clicked(move |_| (st.on_chat)(name.clone()));
                }
                btn_row.append(&b);
            } else {
                let b = Button::builder()
                    .label("安装")
                    .css_classes(vec!["suggested-action"])
                    .build();
                {
                    let st = Rc::clone(st);
                    let full = full.clone();
                    let approx = (m.ref_size_gb * 1e9) as u64;
                    b.connect_clicked(move |_| start_install(&st, &full, Some(approx)));
                }
                btn_row.append(&b);
            }

            let detail_btn = Button::builder().label("详情").css_classes(vec!["flat"]).build();
            {
                let st = Rc::clone(st);
                let m = m.clone();
                detail_btn.connect_clicked(move |_| show_detail(&st, &m));
            }
            btn_row.append(&detail_btn);

            // 对比：勾选状态从 state 恢复（此前重建即丢），超上限给明确提示
            let checked = st.selected.borrow().iter().any(|x| x.id == m.id);
            let cb = CheckButton::builder().label("对比").build();
            cb.set_active(checked);
            {
                let st = Rc::clone(st);
                let m = m.clone();
                let cb_c = cb.clone();
                cb.connect_toggled(move |b| toggle_compare(&st, &m, b.is_active(), &cb_c));
            }
            btn_row.append(&cb);
            box_.append(&btn_row);
        }
        CardItem::Local(mi) => {
            top.append(
                &Label::builder()
                    .label(&mi.name)
                    .halign(gtk4::Align::Start)
                    .css_classes(vec!["heading"])
                    .wrap(true)
                    .build(),
            );
            top.append(&fit_badge(fit));
            box_.append(&top);

            let mut bits: Vec<String> = vec![mi.size_human.clone()];
            if !mi.quant.is_empty() {
                bits.push(mi.quant.clone());
            }
            if !mi.parameter_size.is_empty() {
                bits.push(mi.parameter_size.clone());
            }
            box_.append(
                &Label::builder()
                    .label(bits.join(" · "))
                    .halign(gtk4::Align::Start)
                    .css_classes(vec!["caption", "dim-label"])
                    .build(),
            );
            box_.append(
                &Label::builder()
                    .label("本机已安装；不在精选库中（自行创建或手动拉取的模型）。")
                    .halign(gtk4::Align::Start)
                    .css_classes(vec!["dim-label"])
                    .wrap(true)
                    .build(),
            );

            let btn_row = gtk4::Box::builder()
                .orientation(Orientation::Horizontal)
                .spacing(6)
                .margin_top(4)
                .build();
            let chat_btn = Button::builder()
                .label("对话")
                .css_classes(vec!["suggested-action"])
                .build();
            {
                let st = Rc::clone(st);
                let name = mi.name.clone();
                chat_btn.connect_clicked(move |_| (st.on_chat)(name.clone()));
            }
            btn_row.append(&chat_btn);
            box_.append(&btn_row);
        }
    }

    card.append(&box_);
    card
}

/// 按当前筛选/排序重建卡片网格
fn apply_filter(st: &Rc<DiscoverState>) {
    let q = st.search.borrow().trim().to_string();
    *st.page_cap.borrow_mut() = 60; // 视图变化后重置分页

    // 无搜索词：分类/任务视图
    if q.is_empty() {
        let items = resolve_items(st);
        if items.is_empty() {
            st.empty_label
                .set_label("这个分类下暂时没有条目。\n可切换分类，或到官方库搜索。");
            st.stack.set_visible_child_name("empty");
            update_footer(st, None, "空");
        } else {
            let scope = view_scope(st);
            let n = items.len();
            *st.last_items.borrow_mut() = items.clone();
            render_cards(st, &items);
            st.stack.set_visible_child_name("cards");
            update_footer(st, Some(n), &scope);
        }
        return;
    }

    // 有搜索词：先查本地精选
    let hits: Vec<CardItem> = {
        let cat = st.catalog.borrow();
        cat.search(&q)
            .into_iter()
            .cloned()
            .map(CardItem::Catalog)
            .collect()
    };
    if hits.is_empty() {
        // 空状态给出明确出口：这一点是「云库」名实相符的关键
        st.empty_label.set_label(&format!(
            "精选库里没有匹配「{q}」的模型。\n精选库是离线可用的推荐清单；完整模型请到官方库搜索。"
        ));
        st.stack.set_visible_child_name("empty");
        update_footer(st, Some(0), &format!("搜索「{q}」"));
    } else {
        let n = hits.len();
        *st.last_items.borrow_mut() = hits.clone();
        render_cards(st, &hits);
        st.stack.set_visible_child_name("cards");
        update_footer(st, Some(n), &format!("搜索「{q}」"));
    }
}

/// 取当前视图的条目集合（含排序）
fn resolve_items(st: &Rc<DiscoverState>) -> Vec<CardItem> {
    if let Some(task) = *st.task.borrow() {
        let prof = st.profile.borrow().clone();
        let mut v: Vec<CardItem> = task_models(&st.catalog.borrow(), task, &prof)
            .into_iter()
            .map(CardItem::Catalog)
            .collect();
        apply_sort(st, &mut v);
        return v;
    }

    let cat = *st.category.borrow();

    // 「已安装」= 本机真实条目（含非精选库模型），每个标签一张卡
    if cat == Category::Installed {
        let inst = st.installed.borrow().clone();
        let mut v: Vec<CardItem> = inst.into_iter().map(CardItem::Local).collect();
        apply_sort(st, &mut v);
        return v;
    }

    let names: Vec<String> = st.installed.borrow().iter().map(|m| m.name.clone()).collect();
    let mut v: Vec<CardItem> = st
        .catalog
        .borrow()
        .by_category(cat, &names)
        .into_iter()
        .cloned()
        .map(CardItem::Catalog)
        .collect();
    apply_sort(st, &mut v);
    v
}

/// 排序（v4.0.0：新增白盒评分，并设为默认）
fn apply_sort(st: &Rc<DiscoverState>, items: &mut [CardItem]) {
    let mode = *st.sort.borrow();
    if mode == SortMode::Default {
        // 任务视图已按适配排好；分类视图保持种子顺序（编辑推荐的人工顺序）
        return;
    }
    let prof = st.profile.borrow().clone();
    match mode {
        SortMode::Default => {}
        SortMode::Score => {
            let maxp = st.catalog.borrow().max_pulls();
            let task_cap = (*st.task.borrow()).map(|t| t.cap());
            items.sort_by(|a, b| {
                let sa = score_item(a, task_cap, &prof, maxp);
                let sb = score_item(b, task_cap, &prof, maxp);
                sb.partial_cmp(&sa)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        fit_for(b.size_bytes(), &prof)
                            .rank()
                            .cmp(&fit_for(a.size_bytes(), &prof).rank())
                    })
            });
        }
        SortMode::FitFirst => items.sort_by(|a, b| {
            fit_for(b.size_bytes(), &prof)
                .rank()
                .cmp(&fit_for(a.size_bytes(), &prof).rank())
                .then_with(|| {
                    a.sort_key_size()
                        .partial_cmp(&b.sort_key_size())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        }),
        SortMode::SizeAsc => items.sort_by(|a, b| {
            a.sort_key_size()
                .partial_cmp(&b.sort_key_size())
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        SortMode::SizeDesc => items.sort_by(|a, b| {
            b.sort_key_size()
                .partial_cmp(&a.sort_key_size())
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        SortMode::Name => items.sort_by(|a, b| a.sort_key_name().cmp(&b.sort_key_name())),
    }
}

/// 单卡评分总分（Catalog 条目走三维白盒评分；本机 Local 条目无热度/能力数据，只算适配）
fn score_item(
    item: &CardItem,
    task: Option<Capability>,
    prof: &HardwareProfile,
    maxp: u64,
) -> f64 {
    match item {
        CardItem::Catalog(m) => score_model(m, task, prof, maxp).total,
        CardItem::Local(mi) => match fit_for(mi.size, prof) {
            Fit::Smooth => 30.0,
            Fit::Tight => 22.0,
            Fit::CpuOnly => 8.0,
            Fit::NoFit => -40.0,
        },
    }
}

/// 分页渲染：只画前 page_cap 张，剩余交给「显示更多」
fn render_cards(st: &Rc<DiscoverState>, items: &[CardItem]) {
    let flow = &st.flow;
    while let Some(c) = flow.first_child() {
        flow.remove(&c);
    }
    let cap = *st.page_cap.borrow();
    let show_n = items.len().min(cap);
    for it in &items[..show_n] {
        let card = card_widget(st, it);
        flow.insert(&card, -1);
    }
    let rest = items.len() - show_n;
    if rest > 0 {
        st.more_btn
            .set_label(&format!("显示更多（还有 {rest} 条）"));
    }
    st.more_btn.set_visible(rest > 0);
}

/// 当前视图的描述（用于页脚）
fn view_scope(st: &Rc<DiscoverState>) -> String {
    if let Some(t) = *st.task.borrow() {
        format!("智能筛选：{}", t.label())
    } else {
        format!("分类：{}", st.category.borrow().label())
    }
}

/// 任务导向：筛出对应能力的模型，按适配程度（或体积）排序
fn task_models(cat: &Catalog, task: Task, profile: &HardwareProfile) -> Vec<CatalogModel> {
    let mut v: Vec<CatalogModel> = if task == Task::Weak {
        cat.all().to_vec()
    } else {
        cat.all()
            .iter()
            .filter(|m| m.caps.contains(&task.cap()))
            .cloned()
            .collect()
    };
    if task == Task::Weak {
        v.sort_by(|a, b| a.ref_size_gb.total_cmp(&b.ref_size_gb));
    } else {
        v.sort_by(|a, b| {
            let fa = fit_for((a.ref_size_gb * 1e9) as u64, profile).rank();
            let fb = fit_for((b.ref_size_gb * 1e9) as u64, profile).rank();
            fb.cmp(&fa).then_with(|| {
                a.ref_size_gb
                    .partial_cmp(&b.ref_size_gb)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });
    }
    v
}

// ============================================================
// 状态切换
// ============================================================

fn set_task(st: &Rc<DiscoverState>, task: Option<Task>) {
    *st.task.borrow_mut() = task;
    *st.category.borrow_mut() = Category::All;
    for (t, btn) in st.toggles.borrow().iter() {
        btn.set_active(task == Some(*t));
    }
    apply_filter(st);
}

fn clear_toggles(st: &Rc<DiscoverState>) {
    *st.setting_toggle.borrow_mut() = true;
    for (_, btn) in st.toggles.borrow().iter() {
        btn.set_active(false);
    }
    *st.setting_toggle.borrow_mut() = false;
}

/// 对比勾选：状态持久化到 st.selected，超上限显式回退并提示
fn toggle_compare(st: &Rc<DiscoverState>, m: &CatalogModel, on: bool, cb: &CheckButton) {
    if on {
        let full = {
            let sel = st.selected.borrow();
            sel.len() >= 3 && !sel.iter().any(|x| x.id == m.id)
        };
        if full {
            // 回退勾选框（用守卫避免再次触发本回调）
            *st.setting_toggle.borrow_mut() = true;
            cb.set_active(false);
            *st.setting_toggle.borrow_mut() = false;
            dialogs::info(&st.win, "对比上限", "最多同时对比 3 个模型，请先取消一个再选。");
            return;
        }
    }
    {
        let mut sel = st.selected.borrow_mut();
        if on {
            if !sel.iter().any(|x| x.id == m.id) {
                sel.push(m.clone());
            }
        } else {
            sel.retain(|x| x.id != m.id);
        }
    }
    let n = st.selected.borrow().len();
    st.compare_btn.borrow().set_label(&format!("对比 ({})", n));
}

fn update_footer(st: &Rc<DiscoverState>, shown: Option<usize>, scope: &str) {
    let lib = st.catalog.borrow().len();
    let inst = st.installed.borrow().len();
    let shown_txt = match shown {
        Some(n) => format!("显示 {n} 个"),
        None => "无匹配".to_string(),
    };
    st.footer.set_label(&format!(
        "精选库 {lib} 个 · 本机已装 {inst} 个 · {shown_txt} · {scope}"
    ));
}

fn update_hw_label(st: &Rc<DiscoverState>) {
    let p = st.profile.borrow();
    let note = if p.manual_vram {
        "（显存为手动填写）"
    } else if p.vram_bytes.is_none() {
        "（显存未检测到，按内存估算；点「填显存」可手动指定）"
    } else {
        ""
    };
    st.hw_label
        .set_label(&format!("硬件画像：{}  {}", p.summary(), note));
}

fn redetect_hardware(st: &Rc<DiscoverState>) {
    let p = detect_hardware();
    *st.profile.borrow_mut() = p.clone();
    save_hardware(&p);
    update_hw_label(st);
    apply_filter(st);
    dialogs::info(&st.win, "硬件重测", "已重新检测硬件画像。");
}

fn manual_vram(st: &Rc<DiscoverState>) {
    let cur = st.profile.borrow().vram_gb();
    dialogs::text_input(
        &st.win,
        "手动填写显存",
        "输入你的显卡显存大小（GB），如 8 / 12 / 24：",
        &format!("{:.1}", cur),
        "保存",
        {
            let st = Rc::clone(st);
            move |input| {
                if let Some(s) = input {
                    if let Ok(gb) = s.trim().parse::<f64>() {
                        if gb > 0.0 {
                            let mut p = st.profile.borrow().clone();
                            p.vram_bytes = Some((gb * 1e9) as u64);
                            p.manual_vram = true;
                            *st.profile.borrow_mut() = p.clone();
                            save_hardware(&p);
                            update_hw_label(&st);
                            apply_filter(&st);
                        }
                    }
                }
            }
        },
    );
}

fn refresh_installed(st: &Rc<DiscoverState>) {
    // 重入保护：上一轮未返回则本轮不并发发起（v1.0.4 卡死修复）。
    // 但只是「推迟」而非丢弃——否则下载完成恰好撞上在飞的刷新时，
    // 卡片状态会被吞掉（表现为下载完了卡片还显示安装）。
    static POLL_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if POLL_BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let st_r = Rc::clone(st);
        glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            refresh_installed(&st_r);
        });
        return;
    }
    let client = crate::ollama::client();
    let st_c = Rc::clone(st);
    rt::spawn(
        async move { models::list_models(&client).await.map_err(|e| e.to_string()) },
        move |res: Result<Vec<ModelInfo>, String>| {
            POLL_BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
            match res {
                Ok(ms) => *st_c.installed.borrow_mut() = ms,
                // 服务未运行：已装置空，仍刷新界面
                Err(_) => st_c.installed.borrow_mut().clear(),
            }
            apply_filter(&st_c);
        },
    );
}

// ============================================================
// 安装：确认 + 磁盘检查 + 可取消下载（v3.5.0）
// ============================================================

/// 目标分区的可用字节数（Ollama 模型目录所在分区）。
/// 取不到时返回 None —— 此时不阻止安装，只是不展示剩余空间。
fn free_disk_bytes() -> Option<u64> {
    use std::ffi::CString;
    let dir = {
        let s = crate::config::Settings::load();
        let d = s.ollama_models_dir.trim().to_string();
        if d.is_empty() {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
            std::path::Path::new(&home).join(".ollama").join("models")
        } else {
            std::path::PathBuf::from(d)
        }
    };
    // 目录可能尚未创建，向上找到第一个存在的祖先
    let mut probe = dir;
    let mut guard = 0;
    while !probe.exists() && guard < 64 {
        probe = match probe.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => std::path::PathBuf::from("/"),
        };
        guard += 1;
    }
    let c = CString::new(probe.to_string_lossy().as_bytes()).ok()?;
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return None;
        }
        Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    }
}

/// 安装入口：重复检查 → 磁盘检查 → 确认 → 开始下载
fn start_install(st: &Rc<DiscoverState>, full: &str, size_bytes: Option<u64>) {
    if st.downloads.borrow().iter().any(|d| d == full) {
        dialogs::info(
            &st.win,
            "正在下载",
            &format!("{full} 已在下载中，请等待完成或取消。"),
        );
        return;
    }

    let need = size_bytes.unwrap_or(0);
    let free = free_disk_bytes();
    if need > 0 {
        if let Some(f) = free {
            if f < need {
                dialogs::show_err(
                    &st.win,
                    &format!(
                        "磁盘空间不足，已取消下载。\n\n需要约 {}，目标分区剩余 {}。\n请清理空间后重试。",
                        human_size(need),
                        human_size(f)
                    ),
                );
                return;
            }
        }
    }

    let size_line = if need > 0 {
        format!("预计占用约 {}。", human_size(need))
    } else {
        "体积未知，以下载实际大小为准。".to_string()
    };
    let disk_line = match free {
        Some(f) => format!("目标分区剩余 {}。", human_size(f)),
        None => String::new(),
    };
    let body = format!(
        "将拉取：{full}\n{size_line}{disk_line}\n\n\
         下载时间取决于网速。期间可继续使用应用；在进度窗口点「取消」或关闭该窗口即可中断。"
    );

    let st_c = Rc::clone(st);
    let full_c = full.to_string();
    dialogs::confirm_primary(&st.win, "确认下载模型", &body, "开始下载", move || {
        begin_download(&st_c, &full_c);
    });
}

/// 真正发起下载：登记到 downloads（卡片进入「下载中…」），完成后自动回填状态
fn begin_download(st: &Rc<DiscoverState>, full: &str) {
    if st.downloads.borrow().iter().any(|d| d == full) {
        return;
    }
    st.downloads.borrow_mut().push(full.to_string());
    apply_filter(st);

    let st_cb = Rc::clone(st);
    let full_cb = full.to_string();
    sidebar::pull_flow_with(
        &st.sidebar,
        full,
        Some(Rc::new(move || {
            st_cb.downloads.borrow_mut().retain(|d| d != &full_cb);
            refresh_installed(&st_cb);
        })),
    );
}

// ============================================================
// 官方库检索（并入本页，v3.5.0）
// ============================================================

fn show_official(st: &Rc<DiscoverState>, query: &str) {
    let q = query.trim().to_string();
    st.official_entry.set_text(&q);
    st.stack.set_visible_child_name("official");
    while let Some(c) = st.official_list.first_child() {
        st.official_list.remove(&c);
    }
    if q.is_empty() {
        st.official_status.set_label(
            "输入模型名后点「查询」。官方库需要联网；离线时可用「拉取模型」手动输入完整名称。",
        );
        st.official_entry.grab_focus();
        return;
    }
    st.official_status
        .set_label(&format!("正在查询官方库：{q} …"));

    let client = crate::ollama::client();
    let st_c = Rc::clone(st);
    let q_c = q.clone();
    rt::spawn(
        async move {
            library::fetch_tags(&client, &q_c)
                .await
                .map_err(|e| e.to_string())
        },
        move |res: Result<Vec<library::LibraryTag>, String>| {
            let list = &st_c.official_list;
            while let Some(c) = list.first_child() {
                list.remove(&c);
            }
            match res {
                Ok(tags) if tags.is_empty() => st_c.official_status.set_label(&format!(
                    "官方库没有找到「{q}」的标签，请检查模型名拼写（如 llama3.2、qwen2.5、gemma3）。"
                )),
                Ok(tags) => {
                    st_c.official_status.set_label(&format!(
                        "共 {} 个标签。点「安装」逐个拉取；体积为官方清单值。",
                        tags.len()
                    ));
                    for t in tags {
                        let full = format!("{q}:{}", t.tag);
                        let row = gtk4::Box::builder()
                            .orientation(Orientation::Horizontal)
                            .spacing(8)
                            .build();
                        row.append(
                            &Label::builder()
                                .label(&full)
                                .halign(gtk4::Align::Start)
                                .hexpand(true)
                                .build(),
                        );
                        row.append(&fit_badge(fit_for(t.size, &st_c.profile.borrow())));
                        row.append(
                            &Label::builder()
                                .label(&t.size_human)
                                .css_classes(vec!["caption", "dim-label"])
                                .build(),
                        );
                        if st_c.installed.borrow().iter().any(|m| m.name == full) {
                            row.append(&Image::from_icon_name("object-select-symbolic"));
                            row.append(
                                &Label::builder()
                                    .label("已安装")
                                    .css_classes(vec!["caption"])
                                    .build(),
                            );
                        } else {
                            let b = Button::builder().label("安装").build();
                            {
                                let st_i = Rc::clone(&st_c);
                                let full_i = full.clone();
                                let sz = if t.size > 0 { Some(t.size) } else { None };
                                b.connect_clicked(move |_| start_install(&st_i, &full_i, sz));
                            }
                            row.append(&b);
                        }
                        st_c.official_list.append(&row);
                    }
                }
                Err(e) => st_c.official_status.set_label(&format!(
                    "无法访问官方库：{e}\n可改用「拉取模型」手动输入完整名称（如 qwen2.5:7b）。"
                )),
            }
        },
    );
}

// ============================================================
// 详情对话框（硬件适配 + 量化扫盲）
// ============================================================

fn show_detail(st: &Rc<DiscoverState>, m: &CatalogModel) {
    let win = Window::builder()
        .transient_for(&st.win)
        .modal(true)
        .title(m.display.clone())
        .default_width(720)
        .default_height(620)
        .build();

    let vbox = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(16)
        .margin_end(16)
        .build();

    vbox.append(
        &Label::builder()
            .label(format!("{}  ·  {}", m.display, m.name))
            .halign(gtk4::Align::Start)
            .css_classes(vec!["title-4", "heading"])
            .build(),
    );
    vbox.append(
        &Label::builder()
            .label(&m.desc)
            .halign(gtk4::Align::Start)
            .css_classes(vec!["dim-label"])
            .wrap(true)
            .build(),
    );
    vbox.append(
        &Label::builder()
            .label(format!("能力：{}", m.caps_summary()))
            .halign(gtk4::Align::Start)
            .build(),
    );
    vbox.append(
        &Label::builder()
            .label(format!(
                "中文优化：{}",
                if m.chinese { "是" } else { "否" }
            ))
            .halign(gtk4::Align::Start)
            .build(),
    );

    // 硬件适配（参考）+ 离线可用的安装入口
    // （v3.5.0 修复：此前参考估算行没有安装按钮，离线时详情页是死路）
    vbox.append(
        &Label::builder()
            .label(format!(
                "硬件适配（推荐标签 {} · 体积约 {:.2} GB 参考）",
                m.ref_tag, m.ref_size_gb
            ))
            .halign(gtk4::Align::Start)
            .css_classes(vec!["heading"])
            .margin_top(8)
            .build(),
    );
    let ref_row = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .build();
    ref_row.append(&fit_badge(fit_for(
        (m.ref_size_gb * 1e9) as u64,
        &st.profile.borrow(),
    )));
    ref_row.append(
        &Label::builder()
            .label(format!(
                "{}:{} · 约 {:.2} GB（参考）",
                m.name, m.ref_tag, m.ref_size_gb
            ))
            .css_classes(vec!["caption", "dim-label"])
            .hexpand(true)
            .build(),
    );
    {
        let full = format!("{}:{}", m.name, m.ref_tag);
        let downloading = st.downloads.borrow().iter().any(|d| d == &full);
        let already = st.installed.borrow().iter().any(|x| x.name == full);
        let label = if already {
            "已安装"
        } else if downloading {
            "下载中…"
        } else {
            "安装此标签"
        };
        let btn = Button::builder()
            .label(label)
            .sensitive(!already && !downloading)
            .build();
        if !already && !downloading {
            let st_i = Rc::clone(st);
            let approx = (m.ref_size_gb * 1e9) as u64;
            btn.connect_clicked(move |_| start_install(&st_i, &full, Some(approx)));
        }
        ref_row.append(&btn);
    }
    vbox.append(&ref_row);

    let status = Label::builder()
        .label("联网获取真实标签体积…")
        .css_classes(vec!["caption", "dim-label"])
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    vbox.append(&status);

    let real_box = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .build();
    vbox.append(&real_box);

    let scroll = ScrolledWindow::builder()
        .child(&vbox)
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();

    let close_btn = Button::builder()
        .label("关闭")
        .css_classes(vec!["suggested-action"])
        .build();
    let w = win.clone();
    close_btn.connect_clicked(move |_| w.close());
    let bar = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .halign(gtk4::Align::End)
        .spacing(8)
        .margin_top(8)
        .build();
    bar.append(&close_btn);

    let outer = gtk4::Box::builder().orientation(Orientation::Vertical).build();
    outer.append(&scroll);
    outer.append(&bar);

    win.set_child(Some(&outer));
    win.present();

    // 联网取真实标签体积
    let client = crate::ollama::client();
    let m_name = m.name.clone();
    let m_name_cb = m_name.clone();
    let ref_size = m.ref_size_gb;
    let st_c = Rc::clone(st);
    let real_box_c = real_box.clone();
    let status_c = status.clone();
    rt::spawn(
        async move {
            library::fetch_tags(&client, &m_name)
                .await
                .map_err(|e| e.to_string())
        },
        move |res: Result<Vec<library::LibraryTag>, String>| match res {
            Ok(tags) => {
                status_c.set_label(&format!("真实标签 {} 个，点「安装」拉取：", tags.len()));
                for t in tags {
                    let full = format!("{}:{}", m_name_cb, t.tag);
                    let size = if t.size > 0 {
                        t.size
                    } else {
                        (ref_size * 1e9) as u64
                    };
                    let row = gtk4::Box::builder()
                        .orientation(Orientation::Horizontal)
                        .spacing(8)
                        .build();
                    row.append(
                        &Label::builder()
                            .label(&full)
                            .halign(gtk4::Align::Start)
                            .width_request(200)
                            .build(),
                    );
                    row.append(&fit_badge(fit_for(size, &st_c.profile.borrow())));
                    row.append(
                        &Label::builder()
                            .label(&t.size_human)
                            .css_classes(vec!["caption", "dim-label"])
                            .build(),
                    );
                    let downloading = st_c.downloads.borrow().iter().any(|d| d == &full);
                    let installed = st_c.installed.borrow().iter().any(|x| x.name == full);
                    if installed {
                        row.append(&Image::from_icon_name("object-select-symbolic"));
                        row.append(
                            &Label::builder()
                                .label("已安装")
                                .css_classes(vec!["caption"])
                                .build(),
                        );
                    } else if downloading {
                        row.append(&Button::builder().label("下载中…").sensitive(false).build());
                    } else {
                        let install = Button::builder().label("安装").build();
                        {
                            let st_i = Rc::clone(&st_c);
                            let full_i = full.clone();
                            let sz = if t.size > 0 { Some(t.size) } else { None };
                            install.connect_clicked(move |_| start_install(&st_i, &full_i, sz));
                        }
                        row.append(&install);
                    }
                    real_box_c.append(&row);
                }
            }
            Err(e) => status_c.set_label(&format!(
                "无法联网取真实标签体积（仍可用上方参考值安装）：{e}"
            )),
        },
    );

    vbox.append(&quant_section());
}

/// 量化扫盲段：解释 + 相对体积/质量条形（启发式估算，已标注）
fn quant_section() -> gtk4::Box {
    let box_ = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .margin_top(10)
        .build();
    box_.append(
        &Label::builder()
            .label("量化扫盲（体积/质量为启发式估算，非实测基准）")
            .halign(gtk4::Align::Start)
            .css_classes(vec!["heading"])
            .build(),
    );
    box_.append(
        &Label::builder()
            .label("GGUF 用不同比特压缩权重：fp16 最准但最大；q8_0 几乎无损、体积减半；q4_K_M 在体积与质量间最平衡，是多数人的默认；q3_K / q2_K 更小但质量下降。下方条形为相对体积与质量估算。")
            .halign(gtk4::Align::Start)
            .css_classes(vec!["dim-label"])
            .wrap(true)
            .build(),
    );
    const QUANTS: &[(&str, f64, f64)] = &[
        ("fp16", 1.0, 1.0),
        ("q8_0", 0.5, 0.97),
        ("q5_K_M", 0.38, 0.93),
        ("q4_K_M", 0.32, 0.90),
        ("q3_K_M", 0.26, 0.84),
        ("q2_K", 0.20, 0.78),
    ];
    for (name, size_factor, quality) in QUANTS {
        let row = gtk4::Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(6)
            .build();
        row.append(
            &Label::builder()
                .label(*name)
                .width_request(70)
                .halign(gtk4::Align::Start)
                .build(),
        );
        row.append(
            &Label::builder()
                .label("体积")
                .css_classes(vec!["caption", "dim-label"])
                .build(),
        );
        let size_bar = gtk4::ProgressBar::builder().fraction(*size_factor).build();
        size_bar.set_hexpand(true);
        row.append(&size_bar);
        row.append(
            &Label::builder()
                .label("质量")
                .css_classes(vec!["caption", "dim-label"])
                .build(),
        );
        let q_bar = gtk4::ProgressBar::builder().fraction(*quality).build();
        q_bar.set_hexpand(true);
        row.append(&q_bar);
        box_.append(&row);
    }
    box_
}

// ============================================================
// 为你选（任务导向 starter 包）
// ============================================================

/// 在给定模型集合里挑「能跑且体积最大」的（best fit）
fn best_fit(models: &[CatalogModel], profile: &HardwareProfile) -> Option<CatalogModel> {
    let mut cand: Vec<CatalogModel> = models
        .iter()
        .filter(|m| {
            let f = fit_for((m.ref_size_gb * 1e9) as u64, profile);
            f == Fit::Smooth || f == Fit::Tight
        })
        .cloned()
        .collect();
    cand.sort_by(|a, b| {
        b.ref_size_gb
            .partial_cmp(&a.ref_size_gb)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    cand.first().cloned()
}

fn show_recommend(st: &Rc<DiscoverState>) {
    let profile = st.profile.borrow().clone();
    let all = st.catalog.borrow().all().to_vec();
    let chat_pool: Vec<CatalogModel> = all
        .iter()
        .filter(|m| m.caps.contains(&Capability::Chat))
        .cloned()
        .collect();
    let reason_pool: Vec<CatalogModel> = all
        .iter()
        .filter(|m| m.caps.contains(&Capability::Reason))
        .cloned()
        .collect();
    let embed_pool: Vec<CatalogModel> = all
        .iter()
        .filter(|m| m.caps.contains(&Capability::Embed))
        .cloned()
        .collect();

    let picks: Vec<CatalogModel> = vec![
        best_fit(&chat_pool, &profile).or_else(|| chat_pool.first().cloned()),
        best_fit(&reason_pool, &profile).or_else(|| reason_pool.first().cloned()),
        best_fit(&embed_pool, &profile).or_else(|| embed_pool.first().cloned()),
    ]
    .into_iter()
    .flatten()
    .collect();

    if picks.is_empty() {
        dialogs::info(&st.win, "为你选", "目录为空，无法推荐。");
        return;
    }

    let win = Window::builder()
        .transient_for(&st.win)
        .modal(true)
        .title("为你选 · 起步套餐")
        .default_width(560)
        .build();
    let vbox = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(16)
        .margin_end(16)
        .build();
    vbox.append(
        &Label::builder()
            .label("根据你的硬件，推荐这套起步组合（点安装后有确认步骤）：")
            .halign(gtk4::Align::Start)
            .wrap(true)
            .build(),
    );

    let mut fulls: Vec<String> = Vec::new();
    let maxp = st.catalog.borrow().max_pulls();
    for m in &picks {
        let full = format!("{}:{}", m.name, m.ref_tag);
        fulls.push(full.clone());
        let fit = fit_for((m.ref_size_gb * 1e9) as u64, &profile);
        let row = gtk4::Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(8)
            .build();
        row.append(
            &Label::builder()
                .label(&m.display)
                .halign(gtk4::Align::Start)
                .hexpand(true)
                .build(),
        );
        row.append(&fit_badge(fit));
        row.append(
            &Label::builder()
                .label(format!("约 {:.2} GB", m.ref_size_gb))
                .css_classes(vec!["caption", "dim-label"])
                .build(),
        );
        let install = Button::builder().label("安装").build();
        {
            let st_i = Rc::clone(st);
            let full_i = full.clone();
            let approx = (m.ref_size_gb * 1e9) as u64;
            install.connect_clicked(move |_| start_install(&st_i, &full_i, Some(approx)));
        }
        row.append(&install);
        vbox.append(&row);
        // 白盒推荐理由：最多展示两条（适配 + 热度），全部可解释
        let sb = score_model(m, None, &profile, maxp);
        let reason_txt = sb
            .reasons
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
        vbox.append(
            &Label::builder()
                .label(reason_txt)
                .halign(gtk4::Align::Start)
                .wrap(true)
                .css_classes(vec!["caption", "dim-label"])
                .margin_start(24)
                .build(),
        );
    }

    let install_all = Button::builder()
        .label("安装全部")
        .css_classes(vec!["suggested-action"])
        .build();
    {
        let st_i = Rc::clone(st);
        let fulls = fulls.clone();
        let win_i = win.clone();
        install_all.connect_clicked(move |_| {
            let body = format!(
                "将依次拉取以下 {} 个模型：\n{}\n\n下载可随时在各自进度窗口取消。",
                fulls.len(),
                fulls.join("\n")
            );
            let st_c = Rc::clone(&st_i);
            let fulls_c = fulls.clone();
            dialogs::confirm_primary(&st_i.win, "确认下载全部", &body, "开始下载", move || {
                for f in &fulls_c {
                    begin_download(&st_c, f);
                }
            });
            win_i.close();
        });
    }
    let close = Button::builder().label("关闭").build();
    let w = win.clone();
    close.connect_clicked(move |_| w.close());
    let bar = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(gtk4::Align::End)
        .margin_top(8)
        .build();
    bar.append(&close);
    bar.append(&install_all);
    vbox.append(&bar);

    win.set_child(Some(&vbox));
    win.present();
}

// ============================================================
// 对比视图
// ============================================================

fn show_compare(st: &Rc<DiscoverState>) {
    let sel = st.selected.borrow().clone();
    if sel.is_empty() {
        dialogs::info(&st.win, "对比", "先在卡片上勾选要对比的模型（最多 3 个）。");
        return;
    }
    let profile = st.profile.borrow().clone();

    let win = Window::builder()
        .transient_for(&st.win)
        .modal(true)
        .title("模型对比")
        .default_width(720)
        .default_height(520)
        .build();
    let vbox = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(16)
        .margin_end(16)
        .build();
    vbox.append(
        &Label::builder()
            .label("模型对比（体积/适配为参考值，非实测）")
            .halign(gtk4::Align::Start)
            .css_classes(vec!["caption", "dim-label"])
            .wrap(true)
            .build(),
    );

    let grid = Grid::builder().column_spacing(12).row_spacing(8).build();

    grid.attach(
        &Label::builder()
            .label("项目")
            .halign(gtk4::Align::Start)
            .build(),
        0,
        0,
        1,
        1,
    );
    for (ci, m) in sel.iter().enumerate() {
        grid.attach(
            &Label::builder()
                .label(&m.display)
                .halign(gtk4::Align::Start)
                .css_classes(vec!["heading"])
                .wrap(true)
                .build(),
            (ci + 1) as i32,
            0,
            1,
            1,
        );
    }

    let rows: Vec<(&str, Box<dyn Fn(&CatalogModel) -> String>)> = vec![
        ("参考体积", Box::new(|m| format!("{:.2} GB", m.ref_size_gb))),
        ("推荐标签", Box::new(|m| m.ref_tag.clone())),
        (
            "中文优化",
            Box::new(|m| if m.chinese { "是".into() } else { "否".into() }),
        ),
        ("能力", Box::new(|m| m.caps_summary())),
    ];

    let best_size = sel
        .iter()
        .map(|m| m.ref_size_gb)
        .fold(f64::INFINITY, |a, b| a.min(b));

    for (ri, (label, f)) in rows.iter().enumerate() {
        let r = (ri + 1) as i32;
        grid.attach(
            &Label::builder()
                .label(*label)
                .halign(gtk4::Align::Start)
                .css_classes(vec!["dim-label"])
                .build(),
            0,
            r,
            1,
            1,
        );
        for (ci, m) in sel.iter().enumerate() {
            let val = f(m);
            let highlight = *label == "参考体积" && (m.ref_size_gb - best_size).abs() < 1e-6;
            let l = Label::builder()
                .label(&val)
                .halign(gtk4::Align::Start)
                .wrap(true)
                .build();
            if highlight {
                l.set_markup(&format!("<b>{}</b>", val.replace('&', "&amp;")));
            }
            grid.attach(&l, (ci + 1) as i32, r, 1, 1);
        }
    }

    // 适配行单独画徽章
    let fit_r = (rows.len() + 1) as i32;
    grid.attach(
        &Label::builder()
            .label("对当前硬件")
            .halign(gtk4::Align::Start)
            .css_classes(vec!["dim-label"])
            .build(),
        0,
        fit_r,
        1,
        1,
    );
    for (ci, m) in sel.iter().enumerate() {
        grid.attach(
            &fit_badge(fit_for((m.ref_size_gb * 1e9) as u64, &profile)),
            (ci + 1) as i32,
            fit_r,
            1,
            1,
        );
    }

    let scroll = ScrolledWindow::builder()
        .child(&grid)
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .build();
    vbox.append(&scroll);

    let close = Button::builder()
        .label("关闭")
        .css_classes(vec!["suggested-action"])
        .build();
    let w = win.clone();
    close.connect_clicked(move |_| w.close());
    let bar = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .halign(gtk4::Align::End)
        .spacing(8)
        .margin_top(8)
        .build();
    bar.append(&close);
    vbox.append(&bar);

    win.set_child(Some(&vbox));
    win.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mi(name: &str, size: u64) -> ModelInfo {
        ModelInfo {
            name: name.to_string(),
            model: name.to_string(),
            size,
            size_human: human_size(size),
            parameter_size: "7.6B".to_string(),
            modified_at: String::new(),
            family: "qwen2".to_string(),
            format: "gguf".to_string(),
            quant: "Q4_K_M".to_string(),
        }
    }

    /// v3.5.0 核心修复：已安装状态必须精确到标签。
    /// 旧实现按基名匹配，装了 qwen2.5:0.5b 会让 qwen2.5:7b 也被标为「已安装」。
    #[test]
    fn installed_tags_are_tag_exact() {
        let inst = vec![mi("qwen2.5:0.5b", 400_000_000)];
        let tags = installed_tags_for(&inst, "qwen2.5");
        assert_eq!(tags, vec!["0.5b".to_string()]);
        // 7b 未安装 —— 卡片不应显示已安装
        assert!(!tags.iter().any(|t| t == "7b"));
    }

    #[test]
    fn installed_tags_handles_bare_name_as_latest() {
        let inst = vec![mi("llama3.2", 2_000_000_000)];
        let tags = installed_tags_for(&inst, "llama3.2");
        assert_eq!(tags, vec!["latest".to_string()]);
    }

    #[test]
    fn installed_tags_ignores_other_models() {
        let inst = vec![mi("gemma3:4b", 3_000_000_000)];
        assert!(installed_tags_for(&inst, "qwen2.5").is_empty());
        // 前缀相近但不带冒号的模型名不应误判
        let inst2 = vec![mi("llama3.2x:1b", 1) ];
        assert!(installed_tags_for(&inst2, "llama3.2").is_empty());
    }

    #[test]
    fn sort_mode_index_maps_and_falls_back() {
        assert_eq!(SortMode::from_index(0), SortMode::Score);
        assert_eq!(SortMode::from_index(1), SortMode::Default);
        assert_eq!(SortMode::from_index(2), SortMode::FitFirst);
        assert_eq!(SortMode::from_index(3), SortMode::SizeAsc);
        assert_eq!(SortMode::from_index(4), SortMode::SizeDesc);
        assert_eq!(SortMode::from_index(5), SortMode::Name);
        // 越界回落到默认（推荐评分），不应 panic
        assert_eq!(SortMode::from_index(99), SortMode::Score);
        assert_eq!(SortMode::labels().len(), 6);
    }

    #[test]
    fn card_item_size_uses_real_size_for_local_entries() {
        let local = CardItem::Local(mi("mymodel:latest", 5_000_000_000));
        assert_eq!(local.size_bytes(), 5_000_000_000);
        assert!((local.sort_key_size() - 5.0).abs() < 1e-9);
    }
}
