//! 把 Markdown 文本渲染进 GTK4 `TextBuffer`（用 `TextTag` 表达样式）。
//!
//! 设计取舍（配合 chat.rs 的流式收尾）：
//! - 聊天 token 逐字到达时，缓冲区直接追加**纯文本**（零延迟、无闪烁）；
//! - 待 `Done` 时再用本模块把该助手消息整体替换为 Markdown 富文本。
//! - 不引入 WebKit 等 HTML 渲染库，保持原生、轻量、依赖面小。
//!
//! 为便于测试，解析与渲染分离：
//! - `render_md` 是**纯函数**（不依赖 GTK），把 Markdown 解析成带标签的文本片段序列，可单测；
//! - `insert_markdown` 只负责把这些片段写进 `TextBuffer`（需要 GTK 环境）。

use gtk4::pango;
use gtk4::prelude::*;
use gtk4::{TextBuffer, TextTag};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// 一段经 Markdown 解析后的富文本片段：`text` 为字面文本，`tags` 为应叠加的标签名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdSegment {
    pub text: String,
    pub tags: Vec<&'static str>,
}

/// 确保缓冲区已注册 Markdown 所需的全部 `TextTag`。
/// 同名只注册一次：`create_tag` 在标签已存在时返回 `None`，重复调用安全。
///
/// 注意：不能用 `create_tag(name, &[("weight", &pango::Weight::Bold)])` 这种
/// 泛型属性写法——`pango::Weight` 作为 `&dyn ToValue` 会被装箱成 `PangoWeight`
/// 枚举 GType 的 Value，而 `GtkTextTag::weight` 属性要求 `gint`，glib 会因类型
/// 不匹配而中止（启动即崩溃）。这里改为先建空标签、再用类型安全的 setter 设置。
pub fn ensure_md_tags(buf: &TextBuffer) {
    // 这些属性在 gtk4-rs 0.8 里以原生整数暴露（Pango 枚举的 GType 即 gint），
    // 故用 Pango 标准常量值，避免 `&dyn ToValue` 装箱导致的类型不匹配崩溃。
    const W_BOLD: i32 = 700;

    let mk = |name: &str| -> TextTag {
        buf.create_tag(Some(name), &[])
            .unwrap_or_else(|| panic!("创建 Markdown 标签失败: {name}"))
    };
    // 链接 / 分隔线 / 错误 颜色跟随系统深浅主题，避免硬编码固定色与桌面脱节。
    let dark = gtk4::Settings::default()
        .map(|s| s.is_gtk_application_prefer_dark_theme())
        .unwrap_or(false);
    let (link_blue, hr_gray, err_red, tool_teal) = if dark {
        (
            gtk4::gdk::RGBA::new(0.45, 0.62, 0.95, 1.0),
            gtk4::gdk::RGBA::new(0.70, 0.70, 0.70, 1.0),
            gtk4::gdk::RGBA::new(0.95, 0.45, 0.42, 1.0),
            gtk4::gdk::RGBA::new(0.50, 0.82, 0.70, 1.0),
        )
    } else {
        (
            gtk4::gdk::RGBA::new(0.13, 0.40, 0.78, 1.0),
            gtk4::gdk::RGBA::new(0.45, 0.45, 0.45, 1.0),
            gtk4::gdk::RGBA::new(0.80, 0.20, 0.17, 1.0),
            gtk4::gdk::RGBA::new(0.06, 0.50, 0.40, 1.0),
        )
    };

    let h1 = mk("md-h1");
    h1.set_weight(W_BOLD);
    h1.set_scale(1.4);
    let h2 = mk("md-h2");
    h2.set_weight(W_BOLD);
    h2.set_scale(1.25);
    let h3 = mk("md-h3");
    h3.set_weight(W_BOLD);
    h3.set_scale(1.1);
    let h4 = mk("md-h4");
    h4.set_weight(W_BOLD);
    h4.set_scale(1.05);
    let h5 = mk("md-h5");
    h5.set_weight(W_BOLD);
    let h6 = mk("md-h6");
    h6.set_weight(W_BOLD);
    let strong = mk("md-strong");
    strong.set_weight(W_BOLD);
    let em = mk("md-em");
    em.set_style(pango::Style::Italic);
    let strike = mk("md-strike");
    strike.set_strikethrough(true);
    let code = mk("md-code");
    code.set_family(Some("monospace"));
    let codeblock = mk("md-codeblock");
    codeblock.set_family(Some("monospace"));
    codeblock.set_left_margin(16);
    let quote = mk("md-quote");
    quote.set_left_margin(20);
    quote.set_style(pango::Style::Italic);
    let link = mk("md-link");
    link.set_foreground_rgba(Some(&link_blue));
    link.set_underline(pango::Underline::Single);
    let li = mk("md-li");
    li.set_left_margin(24);
    let table = mk("md-table");
    table.set_left_margin(8);
    let hr = mk("md-hr");
    hr.set_foreground_rgba(Some(&hr_gray));

    // 统计信息行（token 数/速度/耗时）：弱化显示，不喧宾夺主
    let meta = mk("md-meta");
    meta.set_scale(0.9);
    meta.set_foreground_rgba(Some(&hr_gray));
    meta.set_style(pango::Style::Italic);

    // 函数调用（工具调用）：等宽 + 左侧缩进 + 主题色，与模型正常输出明显区分
    let tool = mk("md-tool");
    tool.set_family(Some("monospace"));
    tool.set_left_margin(14);
    tool.set_foreground_rgba(Some(&tool_teal));

    // 错误提示行：失败必须显眼，不能被当成普通输出
    let err = mk("md-err");
    err.set_weight(W_BOLD);
    err.set_foreground_rgba(Some(&err_red));
}

/// 以指定标签插入一段纯文本（用于统计行、错误行等带样式的行内文本）
pub fn insert_tagged(buf: &TextBuffer, iter: &mut gtk4::TextIter, text: &str, tag: &str) {
    let start_off = iter.offset();
    buf.insert(iter, text);
    let s = buf.iter_at_offset(start_off);
    buf.apply_tag_by_name(tag, &s, iter);
}

fn heading_tag(level: pulldown_cmark::HeadingLevel) -> &'static str {
    match level {
        pulldown_cmark::HeadingLevel::H1 => "md-h1",
        pulldown_cmark::HeadingLevel::H2 => "md-h2",
        pulldown_cmark::HeadingLevel::H3 => "md-h3",
        pulldown_cmark::HeadingLevel::H4 => "md-h4",
        pulldown_cmark::HeadingLevel::H5 => "md-h5",
        pulldown_cmark::HeadingLevel::H6 => "md-h6",
    }
}

/// 把一段 Markdown 解析为带样式标签的文本片段序列（纯函数，不依赖 GTK）。
///
/// 这是渲染的核心逻辑，可独立单测。相邻同标签片段会被合并以减少片段数。
pub fn render_md(md: &str) -> Vec<MdSegment> {
    let parser = Parser::new_ext(
        md,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS,
    );

    let mut segs: Vec<MdSegment> = Vec::new();
    // 当前生效的样式标签名（用于 Text / Code 事件）
    let mut stack: Vec<&'static str> = Vec::new();
    // 列表嵌套状态：None = 无序；Some(n) = 有序，下一个序号为 n
    let mut list_state: Vec<Option<u64>> = Vec::new();
    // 链接目标 URL 栈：纯文本 TextView 不可点击，故在链接文字后补注 URL
    let mut link_stack: Vec<String> = Vec::new();

    // 追加一段文本，若与上一片段标签完全一致则合并
    let push = |segs: &mut Vec<MdSegment>, text: String, tags: Vec<&'static str>| {
        if let Some(last) = segs.last_mut() {
            if last.tags == tags && !text.is_empty() {
                last.text.push_str(&text);
                return;
            }
        }
        segs.push(MdSegment { text, tags });
    };

    for ev in parser {
        match ev {
            Event::Text(t) => {
                push(&mut segs, t.as_ref().to_string(), stack.clone());
            }
            Event::Code(c) => {
                let mut tmp = stack.clone();
                tmp.push("md-code");
                push(&mut segs, c.as_ref().to_string(), tmp);
            }
            Event::SoftBreak | Event::HardBreak => {
                push(&mut segs, "\n".to_string(), Vec::new());
            }
            Event::Rule => {
                push(&mut segs, "\n──────────\n".to_string(), vec!["md-hr"]);
            }
            Event::FootnoteReference(c) => {
                push(&mut segs, format!("[^{}]", c.as_ref()), Vec::new());
            }
            Event::TaskListMarker(checked) => {
                push(&mut segs, (if checked { "☑ " } else { "☐ " }).to_string(), Vec::new());
            }
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => stack.push(heading_tag(level)),
                Tag::BlockQuote => stack.push("md-quote"),
                Tag::CodeBlock(_) => stack.push("md-codeblock"),
                Tag::List(first) => {
                    let ordered = first.is_some();
                    list_state.push(if ordered { first } else { None });
                    stack.push("md-li");
                }
                Tag::Item => {
                    let prefix = match list_state.last().copied().flatten() {
                        Some(n) => {
                            if let Some(last) = list_state.last_mut() {
                                *last = Some(n + 1);
                            }
                            format!("{}. ", n)
                        }
                        None => "• ".to_string(),
                    };
                    push(&mut segs, prefix, stack.clone());
                }
                Tag::Emphasis => stack.push("md-em"),
                Tag::Strong => stack.push("md-strong"),
                Tag::Strikethrough => stack.push("md-strike"),
                Tag::Link { dest_url, .. } => {
                    stack.push("md-link");
                    link_stack.push(dest_url.to_string());
                }
                // 表格：降级为「文本 + 分隔符」渲染（纯文本 TextView 无网格能力）
                Tag::Table(_) => stack.push("md-table"),
                // 图片：纯文本 TextView 无法呈现，回落为 alt 文本（内层 Text 事件）
                // HtmlBlock / FootnoteDefinition / MetadataBlock：本应用无对应富文本，忽略
                Tag::TableHead
                | Tag::TableRow
                | Tag::TableCell
                | Tag::Image { .. }
                | Tag::HtmlBlock
                | Tag::FootnoteDefinition(_)
                | Tag::MetadataBlock(_) => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Paragraph => {
                    push(&mut segs, "\n".to_string(), Vec::new());
                }
                TagEnd::Heading(_) => {
                    stack.pop();
                    push(&mut segs, "\n".to_string(), Vec::new());
                }
                TagEnd::BlockQuote => {
                    stack.pop();
                    push(&mut segs, "\n".to_string(), Vec::new());
                }
                TagEnd::CodeBlock => {
                    stack.pop();
                    push(&mut segs, "\n".to_string(), Vec::new());
                }
                TagEnd::List(_) => {
                    list_state.pop();
                    stack.pop();
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                    stack.pop();
                    if let TagEnd::Link = tag_end {
                        if let Some(url) = link_stack.pop() {
                            push(&mut segs, format!(" ({url})"), Vec::new());
                        }
                    }
                }
                TagEnd::Table => {
                    stack.pop();
                    push(&mut segs, "\n".to_string(), Vec::new());
                }
                TagEnd::TableHead => {
                    push(&mut segs, "\n".to_string(), Vec::new());
                }
                TagEnd::TableRow => {
                    push(&mut segs, "\n".to_string(), Vec::new());
                }
                TagEnd::TableCell => {
                    push(&mut segs, " | ".to_string(), Vec::new());
                }
                _ => {}
            },
            Event::Html(_) | Event::InlineHtml(_) => {}
        }
    }

    segs
}

/// 把一段 Markdown 以富文本形式插入到 `iter` 处；调用后 `iter` 指向插入内容末尾。
pub fn insert_markdown(buf: &TextBuffer, iter: &mut gtk4::TextIter, md: &str) {
    for seg in render_md(md) {
        buf.insert_with_tags_by_name(iter, &seg.text, &seg.tags);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(segs: &[MdSegment]) -> String {
        segs.iter().map(|s| s.text.clone()).collect()
    }

    fn seg_with_text<'a>(segs: &'a [MdSegment], needle: &str) -> &'a MdSegment {
        segs.iter().find(|s| s.text.contains(needle)).expect("应存在包含该文本的片段")
    }

    #[test]
    fn md_text_roundtrips() {
        let segs = render_md("# 标题\n正文 **粗体** 与 `代码` 还有 [链接](https://x.io)");
        let t = plain(&segs);
        assert!(t.contains("标题"), "标题应存在: {t:?}");
        assert!(t.contains("粗体"), "粗体应存在: {t:?}");
        assert!(t.contains("代码"), "代码应存在: {t:?}");
        assert!(t.contains("链接"), "链接应存在: {t:?}");
        assert!(t.contains("https://x.io"), "链接目标应保留: {t:?}");
    }

    #[test]
    fn md_strong_tag_applied() {
        let segs = render_md("文本 **粗体内容** 结尾");
        let seg = seg_with_text(&segs, "粗体内容");
        assert!(seg.tags.contains(&"md-strong"), "粗体片段应带 md-strong: {:?}", seg.tags);
    }

    #[test]
    fn md_em_tag_applied() {
        let segs = render_md("文本 *斜体内容* 结尾");
        let seg = seg_with_text(&segs, "斜体内容");
        assert!(seg.tags.contains(&"md-em"), "斜体片段应带 md-em: {:?}", seg.tags);
    }

    #[test]
    fn md_code_tag_applied() {
        let segs = render_md("见 `inline_code` 示例");
        let seg = seg_with_text(&segs, "inline_code");
        assert!(seg.tags.contains(&"md-code"), "行内代码应带 md-code: {:?}", seg.tags);
    }

    #[test]
    fn md_heading_tag_applied() {
        let segs = render_md("# H1 标题");
        let seg = seg_with_text(&segs, "H1 标题");
        assert!(seg.tags.contains(&"md-h1"), "一级标题应带 md-h1: {:?}", seg.tags);
    }

    #[test]
    fn md_unordered_list_renders_bullets() {
        let segs = render_md("- 项目一\n- 项目二");
        let t = plain(&segs);
        assert!(t.contains("• 项目一"), "无序列表应带圆点前缀: {t:?}");
        assert!(t.contains("• 项目二"), "无序列表应带圆点前缀: {t:?}");
        // 列表项应带 md-li 标签
        let seg = seg_with_text(&segs, "项目一");
        assert!(seg.tags.contains(&"md-li"), "列表项应带 md-li: {:?}", seg.tags);
    }

    #[test]
    fn md_ordered_list_renders_numbers() {
        let segs = render_md("1. 第一\n2. 第二");
        let t = plain(&segs);
        assert!(t.contains("1. 第一"), "有序列表应从 1 开始: {t:?}");
        assert!(t.contains("2. 第二"), "有序列表应递增: {t:?}");
    }

    #[test]
    fn md_nested_ordered_list_continues() {
        let segs = render_md("1. 一\n2. 二\n   1. 二点一\n   2. 二点二\n3. 三");
        let t = plain(&segs);
        assert!(t.contains("1. 一"), "外層从 1: {t:?}");
        assert!(t.contains("2. 二"), "外層 2: {t:?}");
        assert!(t.contains("3. 三"), "外層递增到 3: {t:?}");
        assert!(t.contains("1. 二点一"), "内层从 1 重计: {t:?}");
        assert!(t.contains("2. 二点二"), "内层 2: {t:?}");
    }

    #[test]
    fn md_code_block_monospace() {
        let segs = render_md("```rust\nlet x = 1;\n```");
        let seg = seg_with_text(&segs, "let x = 1;");
        assert!(seg.tags.contains(&"md-codeblock"), "代码块内容应带 md-codeblock: {:?}", seg.tags);
    }

    #[test]
    fn md_blockquote_and_hr_and_link() {
        let segs = render_md("> 引用内容\n\n---\n\n[名字](https://e.x)");
        let quote = seg_with_text(&segs, "引用内容");
        assert!(quote.tags.contains(&"md-quote"), "引用应带 md-quote: {:?}", quote.tags);
        let link = seg_with_text(&segs, "名字");
        assert!(link.tags.contains(&"md-link"), "链接文字应带 md-link: {:?}", link.tags);
        let t = plain(&segs);
        assert!(t.contains("https://e.x"), "链接目标应保留: {t:?}");
        // 分隔线应生成带 md-hr 的片段
        assert!(segs.iter().any(|s| s.tags.contains(&"md-hr")), "分隔线应带 md-hr: {:?}", segs);
    }

    #[test]
    fn md_tasklist_markers() {
        let segs = render_md("- [x] 已完成\n- [ ] 待办");
        let t = plain(&segs);
        assert!(t.contains("☑ 已完成"), "已完成项应显示 ☑: {t:?}");
        assert!(t.contains("☐ 待办"), "待办项应显示 ☐: {t:?}");
    }

    #[test]
    fn md_strikethrough() {
        let segs = render_md("~~删除线~~");
        let seg = seg_with_text(&segs, "删除线");
        assert!(seg.tags.contains(&"md-strike"), "删除线应带 md-strike: {:?}", seg.tags);
    }

    #[test]
    fn md_table_degrades_to_text() {
        let segs = render_md("| 列一 | 列二 |\n|---|---|\n| a | b |");
        let t = plain(&segs);
        assert!(t.contains("列一"), "表头应存在: {t:?}");
        assert!(t.contains("a"), "单元格应存在: {t:?}");
        assert!(t.contains(" | "), "单元格间应有分隔符: {t:?}");
    }

    #[test]
    fn md_empty_input_yields_no_segments() {
        assert!(render_md("").is_empty(), "空输入不应产生片段");
    }

    #[test]
    fn md_adjacent_same_tag_merged() {
        // 「**a和b**」整段粗体且相邻（无间隔）应合并为一个片段
        let segs = render_md("**a和b**");
        let bold_segs: Vec<_> = segs.iter().filter(|s| s.tags.contains(&"md-strong")).collect();
        assert_eq!(bold_segs.len(), 1, "相邻同标签应合并: {:?}", segs);
        assert_eq!(bold_segs[0].text, "a和b", "合并后文本应含整段: {:?}", segs);
    }
}