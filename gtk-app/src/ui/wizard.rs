//! Modelfile 引导式创建向导
//!
//! 把原先「纯文本框手写 Modelfile」升级为多页引导：
//! - 基本信息：模型名 + 基座模型（下拉选已装 / 手填 / 选本地权重文件）
//! - 参数：temperature / top_p / top_k / num_ctx / num_gpu（可视化，仅非空才写入）
//! - 提示词：SYSTEM / TEMPLATE 分栏
//! - 高级：ADAPTER / LICENSE
//! 实时预览生成的 Modelfile，确认后调用 `sidebar::create_flow` 真正创建。
//!
//! 说明：keep_alive 是运行时参数而非 Modelfile 字段，故不放进 Modelfile。

use crate::rt;
use crate::ui::sidebar::{create_flow, Sidebar};
use gtk4::prelude::*;
use std::rc::Rc;

/// 把字段整理为 Modelfile 块（多行内容用三引号包裹，保证合法）
fn emit_block(keyword: &str, text: &str) -> String {
    let t = text.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.contains('\n') {
        format!("{} \"\"\"\n{}\n\"\"\"\n", keyword, t)
    } else {
        format!("{} {}\n", keyword, t)
    }
}

fn parse_f32(s: &str) -> Option<f32> {
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        s.parse::<f32>().ok()
    }
}

fn parse_u32(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        s.parse::<u32>().ok()
    }
}

fn parse_i32(s: &str) -> Option<i32> {
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        s.parse::<i32>().ok()
    }
}

/// 依据向导各字段拼出完整 Modelfile 文本
#[allow(clippy::too_many_arguments)]
fn build_modelfile(
    base: &str,
    temp: &str,
    top_p: &str,
    top_k: &str,
    num_ctx: &str,
    num_gpu: &str,
    system: &str,
    template: &str,
    adapter: &str,
    license: &str,
) -> String {
    let mut s = String::new();
    let base = base.trim();
    if !base.is_empty() {
        s.push_str(&format!("FROM {}\n", base));
    }
    if let Some(v) = parse_f32(temp) {
        s.push_str(&format!("PARAMETER temperature {}\n", v));
    }
    if let Some(v) = parse_f32(top_p) {
        s.push_str(&format!("PARAMETER top_p {}\n", v));
    }
    if let Some(v) = parse_u32(top_k) {
        s.push_str(&format!("PARAMETER top_k {}\n", v));
    }
    if let Some(v) = parse_u32(num_ctx) {
        s.push_str(&format!("PARAMETER num_ctx {}\n", v));
    }
    if let Some(v) = parse_i32(num_gpu) {
        s.push_str(&format!("PARAMETER num_gpu {}\n", v));
    }
    s.push_str(&emit_block("SYSTEM", system));
    s.push_str(&emit_block("TEMPLATE", template));
    s.push_str(&emit_block("ADAPTER", adapter));
    s.push_str(&emit_block("LICENSE", license));
    s
}

/// 创建模型向导（v1.0.22 重做）：改用通用分步向导框架。
/// 旧版用 Notebook 四个标签页——名为向导实无引导：任意跳页、无步骤感，
/// 且校验全压在最后的「创建」上（缺基座模型才弹窗让人回去翻页）。
/// 现为线性 5 步：基本信息 → 参数 → 提示词 → 高级 → 确认创建，
/// 每步离开前就地校验（红字提示，不弹窗），Modelfile 预览随填写实时刷新。
pub fn open_modelfile_wizard(parent: &gtk4::Window, st: &Rc<Sidebar>) {
    use crate::ui::stepper::StepWizard;

    let w = StepWizard::new(parent, "创建模型", 660, 640);
    let host = w.window();

    // ---------------- 步骤 1：基本信息 ----------------
    let page1 = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(8)
        .margin_start(2)
        .margin_end(2)
        .build();
    page1.append(&labeled("模型名称", "创建后的模型名，如 my-helper"));
    let name_entry = gtk4::Entry::builder()
        .hexpand(true)
        .placeholder_text("如 my-helper")
        .build();
    page1.append(&name_entry);

    let base_lbl = gtk4::Label::builder()
        .label("基座模型（选已装 / 手填 / 选本地权重文件）")
        .halign(gtk4::Align::Start)
        .margin_top(8)
        .build();
    page1.append(&base_lbl);
    let base_store = gtk4::StringList::new(&["（手填或选本地文件）"]);
    let base_dd = gtk4::DropDown::builder().model(&base_store).hexpand(true).build();
    let base_entry = gtk4::Entry::builder()
        .hexpand(true)
        .placeholder_text("如 llama3.2:latest 或 /path/to/model.gguf")
        .build();
    let file_btn = gtk4::Button::builder()
        .label("选择本地权重文件")
        .css_classes(vec!["flat"])
        .halign(gtk4::Align::Start)
        .build();
    page1.append(&base_dd);
    page1.append(&base_entry);
    page1.append(&file_btn);

    // 异步填入已安装模型到下拉
    {
        let base_store = base_store.clone();
        rt::spawn(
            async move {
                crate::ollama::models::list_models(&crate::ollama::client())
                    .await
                    .map_err(|e| e.to_string())
            },
            move |res: Result<Vec<crate::ollama::ModelInfo>, String>| {
                if let Ok(models) = res {
                    for m in models {
                        base_store.append(&m.name);
                    }
                }
            },
        );
    }
    // 选下拉项 → 填入 base_entry（index 0 为占位，不填）
    {
        let base_store = base_store.clone();
        let base_entry = base_entry.clone();
        base_dd.connect_selected_notify(move |dd| {
            let i = dd.selected();
            if i > 0 {
                if let Some(n) = base_store.string(i) {
                    base_entry.set_text(&n);
                }
            }
        });
    }
    // 选本地权重文件 → 填入路径
    {
        let win = host.clone();
        let base_entry = base_entry.clone();
        file_btn.connect_clicked(move |_| {
            let dlg = gtk4::FileChooserNative::new(
                Some("选择本地权重文件"),
                Some(&win),
                gtk4::FileChooserAction::Open,
                Some("打开"),
                Some("取消"),
            );
            let filter = gtk4::FileFilter::new();
            filter.add_pattern("*.gguf");
            filter.add_pattern("*.bin");
            filter.add_pattern("*.safetensors");
            filter.set_name(Some("权重文件 (*.gguf *.bin *.safetensors)"));
            dlg.set_filter(&filter);
            // v3.1.1：引用保活，防 show 后销毁段错误（同 chat.rs）
            let dlg_keep = dlg.clone();
            let base_entry = base_entry.clone();
            dlg.connect_response(move |chooser, resp| {
                let _alive = &dlg_keep;
                if resp == gtk4::ResponseType::Accept {
                    if let Some(file) = chooser.file() {
                        if let Some(p) = file.path() {
                            base_entry.set_text(&p.to_string_lossy());
                        }
                    }
                }
            });
            dlg.show();
        });
    }

    // ---------------- 步骤 2：参数 ----------------
    let page2 = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(6)
        .margin_start(2)
        .margin_end(2)
        .build();
    let hint2 = gtk4::Label::builder()
        .label("留空即使用模型默认值，只有填写的字段才会写入 Modelfile。")
        .css_classes(vec!["dim-label"])
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    hint2.set_xalign(0.0);
    page2.append(&hint2);
    let mk_field = |label: &str, placeholder: &str| -> gtk4::Entry {
        let l = gtk4::Label::builder()
            .label(label)
            .halign(gtk4::Align::Start)
            .width_request(140)
            .build();
        let e = gtk4::Entry::builder().hexpand(true).placeholder_text(placeholder).build();
        let row = gtk4::Box::builder().orientation(gtk4::Orientation::Horizontal).spacing(8).build();
        row.append(&l);
        row.append(&e);
        page2.append(&row);
        e
    };
    let t_entry = mk_field("temperature", "如 0.7");
    let tp_entry = mk_field("top_p", "如 0.9");
    let tk_entry = mk_field("top_k", "如 40");
    let ctx_entry = mk_field("num_ctx", "如 4096");
    let gpu_entry = mk_field("num_gpu", "如 1（0 = 用 CPU）");

    // ---------------- 步骤 3：提示词 ----------------
    let page3 = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(8)
        .margin_start(2)
        .margin_end(2)
        .build();
    let sys_lbl = gtk4::Label::builder().label("SYSTEM 系统提示").halign(gtk4::Align::Start).build();
    let sys_buf = gtk4::TextBuffer::builder().build();
    let sys_view = gtk4::TextView::builder()
        .buffer(&sys_buf)
        .height_request(90)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .css_classes(vec!["monospace"])
        .build();
    let sys_scroll = gtk4::ScrolledWindow::builder().child(&sys_view).min_content_height(90).build();
    page3.append(&sys_lbl);
    page3.append(&sys_scroll);
    let tmpl_lbl = gtk4::Label::builder()
        .label("TEMPLATE 模板（多行自动用三引号包裹）")
        .halign(gtk4::Align::Start)
        .build();
    let tmpl_buf = gtk4::TextBuffer::builder().build();
    let tmpl_view = gtk4::TextView::builder()
        .buffer(&tmpl_buf)
        .height_request(130)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .css_classes(vec!["monospace"])
        .build();
    let tmpl_scroll = gtk4::ScrolledWindow::builder().child(&tmpl_view).min_content_height(130).build();
    page3.append(&tmpl_lbl);
    page3.append(&tmpl_scroll);

    // ---------------- 步骤 4：高级 ----------------
    let page4 = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(8)
        .margin_start(2)
        .margin_end(2)
        .build();
    let adapter_lbl = gtk4::Label::builder().label("ADAPTER 适配器路径（可选）").halign(gtk4::Align::Start).build();
    let adapter_entry = gtk4::Entry::builder().hexpand(true).placeholder_text("如 /path/to/adapter.bin").build();
    page4.append(&adapter_lbl);
    page4.append(&adapter_entry);
    let lic_lbl = gtk4::Label::builder()
        .label("LICENSE 许可证（可选，多行自动用三引号包裹）")
        .halign(gtk4::Align::Start)
        .build();
    let lic_buf = gtk4::TextBuffer::builder().build();
    let lic_view = gtk4::TextView::builder()
        .buffer(&lic_buf)
        .height_request(110)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .css_classes(vec!["monospace"])
        .build();
    let lic_scroll = gtk4::ScrolledWindow::builder().child(&lic_view).min_content_height(110).build();
    page4.append(&lic_lbl);
    page4.append(&lic_scroll);

    // ---------------- 步骤 5：确认（实时预览） ----------------
    let page5 = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(6)
        .margin_start(2)
        .margin_end(2)
        .build();
    let hint5 = gtk4::Label::builder()
        .label("下面是即将提交的 Modelfile，随前面的填写实时更新；确认无误点「创建」。")
        .css_classes(vec!["dim-label"])
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    hint5.set_xalign(0.0);
    let prev_buf = gtk4::TextBuffer::builder().build();
    let prev_view = gtk4::TextView::builder()
        .buffer(&prev_buf)
        .editable(false)
        .cursor_visible(false)
        .height_request(240)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .css_classes(vec!["monospace"])
        .build();
    let prev_scroll = gtk4::ScrolledWindow::builder()
        .child(&prev_view)
        .min_content_height(200)
        .vexpand(true)
        .build();
    page5.append(&hint5);
    page5.append(&prev_scroll);

    // ---- 读取全部字段 → (模型名, Modelfile) ----
    let read = {
        let name_entry = name_entry.clone();
        let base_entry = base_entry.clone();
        let t_entry = t_entry.clone();
        let tp_entry = tp_entry.clone();
        let tk_entry = tk_entry.clone();
        let ctx_entry = ctx_entry.clone();
        let gpu_entry = gpu_entry.clone();
        let sys_buf = sys_buf.clone();
        let tmpl_buf = tmpl_buf.clone();
        let adapter_entry = adapter_entry.clone();
        let lic_buf = lic_buf.clone();
        move || -> (String, String) {
            let name = name_entry.text().trim().to_string();
            let base = base_entry.text().to_string();
            let mf = build_modelfile(
                &base,
                &t_entry.text(),
                &tp_entry.text(),
                &tk_entry.text(),
                &ctx_entry.text(),
                &gpu_entry.text(),
                &sys_buf.text(&sys_buf.start_iter(), &sys_buf.end_iter(), false),
                &tmpl_buf.text(&tmpl_buf.start_iter(), &tmpl_buf.end_iter(), false),
                &adapter_entry.text(),
                &lic_buf.text(&lic_buf.start_iter(), &lic_buf.end_iter(), false),
            );
            (name, mf)
        }
    };

    // ---- 预览实时刷新：任一字段变动即更新（旧版要点「刷新预览」才更新） ----
    let refresh: Rc<dyn Fn()> = Rc::new({
        let read = read.clone();
        let prev_buf = prev_buf.clone();
        move || {
            let (_, mf) = read();
            prev_buf.set_text(&mf);
        }
    });
    for e in [&name_entry, &base_entry, &t_entry, &tp_entry, &tk_entry, &ctx_entry, &gpu_entry, &adapter_entry] {
        let rf = refresh.clone();
        e.connect_changed(move |_| rf());
    }
    for b in [&sys_buf, &tmpl_buf, &lic_buf] {
        let rf = refresh.clone();
        b.connect_changed(move |_| rf());
    }
    refresh();

    // ---- 逐步校验：离开该步前就地提示，不弹窗 ----
    {
        let name_entry = name_entry.clone();
        let base_entry = base_entry.clone();
        w.add_page(
            "基本信息",
            "先给它一个名字，并指定从哪个模型派生。",
            &page1,
            Some(Rc::new(move || {
                if name_entry.text().trim().is_empty() {
                    return Some("请填写模型名称".into());
                }
                if base_entry.text().trim().is_empty() {
                    return Some("请填写基座模型：从下拉选已装模型、手填名称，或点「选择本地权重文件」".into());
                }
                None
            })),
        );
    }
    {
        let fields: Vec<(String, gtk4::Entry, bool)> = vec![
            ("temperature".into(), t_entry.clone(), true),
            ("top_p".into(), tp_entry.clone(), true),
            ("top_k".into(), tk_entry.clone(), false),
            ("num_ctx".into(), ctx_entry.clone(), false),
            ("num_gpu".into(), gpu_entry.clone(), false),
        ];
        w.add_page(
            "参数",
            "留空即用模型默认值。",
            &page2,
            Some(Rc::new(move || {
                for (label, entry, is_float) in &fields {
                    let t = entry.text();
                    let t = t.trim();
                    if t.is_empty() {
                        continue;
                    }
                    let bad = if *is_float {
                        parse_f32(t).is_none()
                    } else {
                        parse_u32(t).is_none()
                    };
                    if bad {
                        return Some(format!("{label} 需要填数字（当前是「{t}」），不用就留空"));
                    }
                }
                None
            })),
        );
    }
    w.add_page(
        "提示词",
        "给模型定性格与输出格式。",
        &page3,
        None,
    );
    w.add_page(
        "高级",
        "适配器与许可证，一般用不到。",
        &page4,
        None,
    );
    w.add_page(
        "确认",
        "检查生成的 Modelfile，然后创建。",
        &page5,
        None,
    );

    // ---- 创建 ----
    {
        let read = read.clone();
        let st_c = Rc::clone(st);
        w.on_finish("创建", move || {
            let (name, mf) = read();
            if name.is_empty() {
                return Some("请填写模型名称".into());
            }
            if mf.trim().is_empty() || !mf.starts_with("FROM ") {
                return Some("缺少基座模型，请回到第 1 步填写".into());
            }
            create_flow(&st_c, &name, &mf);
            None
        });
    }

    w.present();
}

/// 小标题（分步向导内的字段标签）
fn labeled(text: &str, desc: &str) -> gtk4::Label {
    let l = gtk4::Label::builder()
        .label(&format!("{text}\n<span size='small'>{desc}</span>"))
        .halign(gtk4::Align::Start)
        .use_markup(true)
        .wrap(true)
        .build();
    l.set_xalign(0.0);
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_modelfile_includes_from_and_parameters() {
        let mf = build_modelfile(
            "  llama3.2 ",   // base（含前后空格应被 trim）
            "0.7",           // temperature
            "0.9",           // top_p
            "40",            // top_k
            "4096",          // num_ctx
            "1",             // num_gpu
            "",              // system 空
            "",              // template 空
            "",              // adapter 空
            "",              // license 空
        );
        assert!(mf.starts_with("FROM llama3.2\n"), "应以 trim 后的 FROM 开头");
        assert!(mf.contains("PARAMETER temperature 0.7\n"));
        assert!(mf.contains("PARAMETER top_p 0.9\n"));
        assert!(mf.contains("PARAMETER top_k 40\n"));
        assert!(mf.contains("PARAMETER num_ctx 4096\n"));
        assert!(mf.contains("PARAMETER num_gpu 1\n"));
    }

    #[test]
    fn build_modelfile_omits_empty_params_and_blocks() {
        let mf = build_modelfile(
            "phi4",
            "", "", "", "", "",   // 全部参数留空 → 不应出现任何 PARAMETER
            "", "", "", "",       // system/template/adapter/license 空
        );
        assert!(mf.starts_with("FROM phi4\n"));
        assert!(!mf.contains("PARAMETER"), "空参数不应生成 PARAMETER 行");
        assert!(!mf.contains("SYSTEM"));
        assert!(!mf.contains("TEMPLATE"));
    }

    #[test]
    fn build_modelfile_multi_line_block_is_quoted() {
        let mf = build_modelfile(
            "qwen2.5",
            "", "", "", "", "",
            "你是一个助手\n请用中文回答", // 多行 system → 三引号包裹
            "", "", "",
        );
        assert!(mf.contains("SYSTEM \"\"\"\n你是一个助手\n请用中文回答\n\"\"\"\n"), "多行 SYSTEM 须用三引号包裹");
    }

    #[test]
    fn build_modelfile_empty_base_yields_no_from() {
        let mf = build_modelfile("", "", "", "", "", "", "", "", "", "");
        assert!(!mf.contains("FROM"), "空基座不应生成 FROM 行");
    }

    #[test]
    fn emit_block_single_line_and_empty() {
        assert_eq!(emit_block("SYSTEM", "   "), "", "纯空白块应为空");
        assert_eq!(emit_block("LICENSE", "MIT"), "LICENSE MIT\n", "单行块不带引号");
    }
}
