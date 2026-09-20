//! 左侧模型列表侧边栏：模型列表 + 选中聊天 + 刷新 + 拉取 + 右键操作
//! 操作（详情/复制/删除/拉取）用对话框实现。

use crate::ollama::{self, ModelInfo};
use crate::rt;
use crate::ui::dialogs;
use gtk4::prelude::*;
use gtk4::{ListBox, ListBoxRow, Orientation, ScrolledWindow};
use std::cell::RefCell;
use std::rc::Rc;

/// 拉取任务进行中标志（退出守门用：有拉取时不允许静默退出）
pub static PULL_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub struct Sidebar {
    pub list: ListBox,
    pub names: RefCell<Vec<String>>,
    pub status: gtk4::Label,
    /// 全量模型行（名称, 行）用于搜索过滤
    pub all_rows: RefCell<Vec<(String, ListBoxRow)>>,
    /// 搜索框
    pub search: gtk4::SearchEntry,
    /// 主窗口（对话框父窗口）
    pub win: gtk4::Window,
}

/// 载入模型列表（清空重建）
pub fn reload(st: &Rc<Sidebar>) {
    let st = Rc::clone(st);
    // 必须用 ollama::client()：它读取用户配置的 base_url。
    // 此前用 OllamaClient::default() 会固定打到 localhost:11434，
    // 用户改了服务地址后模型列表仍查旧地址（设置→连接生效环漏洞）。
    let client = ollama::client();
    rt::spawn(
        async move {
            let models = match ollama::models::list_models(&client).await {
                Ok(m) => m,
                Err(_) => Vec::new(),
            };
            // v1.0.10：探测未缓存模型的能力（本地调用，毫秒级），用于标记仅嵌入模型。
            // 探测失败不缓存——不得据此过滤（诚实边界）。
            for m in &models {
                if ollama::models::cached_is_embed_only(&m.name).is_none() {
                    if let Some(caps) = ollama::models::model_capabilities(&client, &m.name).await {
                        ollama::models::cache_capabilities(&m.name, &caps);
                    }
                }
            }
            models
        },
        move |models: Vec<ModelInfo>| {
            while let Some(row) = st.list.row_at_index(0) {
                st.list.remove(&row);
            }
            st.names.borrow_mut().clear();
            st.all_rows.borrow_mut().clear();
            for m in &models {
                st.names.borrow_mut().push(m.name.clone());
                let embed_only = ollama::models::cached_is_embed_only(&m.name) == Some(true);
                let row = model_row(&m, embed_only);
                st.all_rows.borrow_mut().push((m.name.clone(), row.clone()));
                st.list.append(&row);
            }
            st.status.set_label(&format!("共 {} 个模型", models.len()));
            // 应用当前搜索过滤
            apply_search(&st);
        },
    );
}

/// 按搜索框文本过滤列表
fn apply_search(st: &Rc<Sidebar>) {
    let q = st.search.text().to_lowercase();
    // 清空列表再按匹配重建
    while let Some(row) = st.list.row_at_index(0) {
        st.list.remove(&row);
    }
    for (name, row) in st.all_rows.borrow().iter() {
        if q.is_empty() || name.to_lowercase().contains(&q) {
            st.list.append(row);
        }
    }
}

/// 构建侧边栏。`parent` 为主窗口；`on_chat(name)` 用户对某模型发起聊天时回调。
pub fn build_sidebar<F>(parent: &gtk4::Window, on_chat: F) -> (gtk4::Box, Rc<Sidebar>)
where
    F: Fn(String) + 'static,
{
    let list = ListBox::builder()
        .css_classes(vec!["navigation-sidebar"])
        .width_request(250)
        .build();

    let scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();

    let vbox = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(6)
        .build();

    // 顶部标题 + 刷新 + 拉取
    let head = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .margin_top(10)
        .margin_start(10)
        .margin_end(10)
        .build();
    let title = gtk4::Label::builder()
        .label("模型")
        .css_classes(vec!["title-4", "heading"])
        .halign(gtk4::Align::Start)
        .hexpand(true)
        .build();
    let refresh_btn = gtk4::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("刷新")
        .build();
    let pull_btn = gtk4::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("拉取新模型")
        .build();
    let create_btn = gtk4::Button::builder()
        .icon_name("document-new-symbolic")
        .tooltip_text("从 Modelfile 创建模型")
        .build();
    let library_btn = gtk4::Button::builder()
        .icon_name("folder-symbolic")
        .tooltip_text("浏览模型库")
        .build();
    head.append(&title);
    head.append(&refresh_btn);
    head.append(&pull_btn);
    head.append(&create_btn);
    head.append(&library_btn);
    vbox.append(&head);

    // 搜索框（按模型名过滤）
    let search = gtk4::SearchEntry::builder()
        .placeholder_text("搜索模型…")
        .margin_start(10)
        .margin_end(10)
        .margin_bottom(4)
        .build();
    vbox.append(&search);

    let status = gtk4::Label::builder()
        .css_classes(vec!["dim-label"])
        .halign(gtk4::Align::Start)
        .margin_start(12)
        .margin_bottom(8)
        .build();
    vbox.append(&scroll);
    vbox.append(&status);

    let state = Rc::new(Sidebar {
        list,
        names: RefCell::new(Vec::new()),
        status: status.clone(),
        all_rows: RefCell::new(Vec::new()),
        search: search.clone(),
        win: parent.clone(),
    });

    // 搜索事件：文本变化时过滤
    {
        let st = Rc::clone(&state);
        search.connect_search_changed(move |_| {
            apply_search(&st);
        });
    }

    // 刷新
    {
        let st = Rc::clone(&state);
        refresh_btn.connect_clicked(move |_| reload(&st));
    }

    // 拉取
    {
        let st = Rc::clone(&state);
        pull_btn.connect_clicked(move |_| {
            let st = Rc::clone(&st);
            let win = st.win.clone();
            dialogs::text_input(
                &win,
                "拉取模型",
                "输入要拉取的模型名称，如 llama3.2:1b",
                "",
                "开始拉取",
                move |input| {
                    if let Some(name) = input {
                        let name = name.trim().to_string();
                        if !name.is_empty() {
                            pull_flow(&st, &name);
                        }
                    }
                },
            );
        });
    }

    // 创建（Modelfile 引导向导）
    {
        let st = Rc::clone(&state);
        let win = st.win.clone();
        create_btn.connect_clicked(move |_| {
            let st = Rc::clone(&st);
            let win = win.clone();
            crate::ui::wizard::open_modelfile_wizard(&win, &st);
        });
    }

    // 浏览模型库
    {
        let st = Rc::clone(&state);
        library_btn.connect_clicked(move |_| {
            let st = Rc::clone(&st);
            let win = st.win.clone();
            dialogs::library_browser(&win, move |name| {
                let st = Rc::clone(&st);
                pull_flow(&st, &name);
            });
        });
    }

    // 首次载入
    reload(&state);

    // 单击选中行：聊天
    let on_chat: Rc<dyn Fn(String)> = Rc::new(on_chat);
    let list = state.list.clone();
    list.connect_row_selected(glib::clone!(@strong state, @strong on_chat => move |_, row| {
        if let Some(row) = row {
            let idx = row.index();
            if idx >= 0 {
                let name = state.names.borrow()[idx as usize].clone();
                on_chat(name);
            }
        }
    }));

    // 右键：模型操作菜单
    let list2 = state.list.clone();
    let gesture = gtk4::GestureClick::new();
    gesture.set_button(3);
    gesture.connect_pressed(glib::clone!(@strong state, @strong on_chat => move |_, _, _, y| {
        let Some(row) = state.list.row_at_y(y as i32) else { return };
        let idx = row.index();
        if idx < 0 {
            return;
        }
        let name = state.names.borrow()[idx as usize].clone();
        state.list.select_row(Some(&row));
        show_model_actions(&state, &name, Rc::clone(&on_chat));
    }));
    list2.add_controller(gesture);

    (vbox, state)
}

fn model_row(m: &ModelInfo, embed_only: bool) -> ListBoxRow {
    let row = ListBoxRow::new();
    let b = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .margin_start(10)
        .margin_end(10)
        .margin_top(6)
        .margin_bottom(6)
        .spacing(1)
        .build();
    // 仅嵌入模型在名字后标注（v1.0.10：防止误选进对话/生成）
    let name_label = if embed_only {
        format!("{}  · 嵌入", m.name)
    } else {
        m.name.clone()
    };
    let n = gtk4::Label::builder().label(&name_label).halign(gtk4::Align::Start).build();
    let meta = gtk4::Label::builder()
        .label(format!("{} · {}", m.parameter_size, m.size_human))
        .halign(gtk4::Align::Start)
        .css_classes(vec!["caption", "dim-label"])
        .build();
    b.append(&n);
    b.append(&meta);
    row.set_child(Some(&b));
    row
}

/// 模型右键动作：聊天/详情/复制/删除（Popover + Button 列表）
fn show_model_actions(st: &Rc<Sidebar>, name: &str, on_chat: Rc<dyn Fn(String)>) {
    let win = st.win.clone();
    let name = name.to_string();

    // 弹出一个含动作按钮的 Popover（右侧定位到鼠标处）
    let pop = gtk4::Popover::new();
    let parent_widget: gtk4::Widget = st.list.clone().upcast();
    pop.set_parent(&parent_widget);

    let vb = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(2)
        .margin_top(4)
        .margin_bottom(4)
        .build();

    let btn_chat = gtk4::Button::builder().label("聊天").halign(gtk4::Align::Start).css_classes(vec!["flat"]).build();
    let btn_detail = gtk4::Button::builder().label("详情").halign(gtk4::Align::Start).css_classes(vec!["flat"]).build();
    let btn_copy = gtk4::Button::builder().label("复制").halign(gtk4::Align::Start).css_classes(vec!["flat"]).build();
    let btn_push = gtk4::Button::builder().label("推送").halign(gtk4::Align::Start).css_classes(vec!["flat"]).build();
    let btn_del = gtk4::Button::builder().label("删除").halign(gtk4::Align::Start).css_classes(vec!["flat"]).build();

    vb.append(&btn_chat);
    vb.append(&btn_detail);
    vb.append(&btn_copy);
    vb.append(&btn_push);
    vb.append(&btn_del);
    pop.set_child(Some(&vb));
    pop.popup();

    // 聊天
    let pop_c2 = pop.clone();
    let n = name.clone();
    btn_chat.connect_clicked(move |_| {
        on_chat(n.clone());
        pop_c2.popdown();
    });

    // 详情
    let pop_d = pop.clone();
    let win_d = win.clone();
    let n_d = name.clone();
    btn_detail.connect_clicked(move |_| {
        pop_d.popdown();
        show_detail(&win_d, &n_d);
    });

    // 复制
    let pop_cp = pop.clone();
    let win_cp = win.clone();
    let n_cp = name.clone();
    let st_cp = Rc::clone(st);
    btn_copy.connect_clicked(move |_| {
        pop_cp.popdown();
        dialogs::text_input(&win_cp, "复制模型", "目标名称", &n_cp, "复制", {
            let win = win_cp.clone();
            let n = n_cp.clone();
            let st = Rc::clone(&st_cp);
            move |dest| {
                if let Some(dest) = dest {
                    let dest = dest.trim().to_string();
                    if !dest.is_empty() {
                        let win2 = win.clone();
                        let client = ollama::client();
                        let n2 = n.clone();
                        let st2 = Rc::clone(&st);
                        rt::spawn(
                            async move { ollama::models::copy_model(&client, &n2, &dest).await },
                            move |res| {
                                match res {
                                    Ok(()) => dialogs::info(&win2, "复制完成", "模型已复制"),
                                    Err(e) => dialogs::show_err(&win2, &e.to_string()),
                                }
                                reload(&st2);
                            },
                        );
                    }
                }
            }
        });
    });

    // 推送
    let pop_p = pop.clone();
    let win_p = win.clone();
    let n_p = name.clone();
    let st_p = Rc::clone(st);
    btn_push.connect_clicked(move |_| {
        pop_p.popdown();
        let st = Rc::clone(&st_p);
        let win = win_p.clone();
        let n = n_p.clone();
        let pd = dialogs::progress(&win, "推送模型", true);
        pd.set_text(&format!("正在推送 {n}…（完成后自动关闭）"));
        let client = ollama::client();
        let pd2 = Rc::clone(&pd);
        let pd_done = Rc::clone(&pd);
        let n2 = n.clone();
        let n_done = n.clone();
        let st2 = Rc::clone(&st);
        let win2 = win.clone();
        let cancel2 = pd.cancel_flag();
        rt::spawn_stream(
            move |tx| {
                crate::rt::block_on(async move {
                    let res = ollama::pull::push_model(&client, &n2, cancel2, |pct, total, completed| {
                        let txt = if pct >= 0.0 {
                            format!("{}{}|{}/{} 字节", PROG_PREFIX, pct, completed, total)
                        } else {
                            format!("{}|正在推送…", PROG_PREFIX)
                        };
                        let _ = tx.send(crate::rt::StreamMsg::Value(txt));
                    }).await;
                    // 推送/创建/拉取无生成统计，填默认值以匹配统一的 StreamMsg 类型
                    let _ = tx.send(crate::rt::StreamMsg::Done(
                        res.map(|_| crate::ollama::GenStats::default()).map_err(|e| e.to_string()),
                    ));
                });
            },
            move |msg| {
                if let crate::rt::StreamMsg::Value(s) = msg {
                    handle_progress(&pd2, &s);
                }
            },
            move |msg: crate::rt::StreamMsg| {
                let cancelled = pd_done.is_cancelled();
                pd_done.close();
                match msg {
                    crate::rt::StreamMsg::Value(_) => {}
                    crate::rt::StreamMsg::Think(_) => {}
                    crate::rt::StreamMsg::Done(Ok(_)) => {
                        dialogs::info(&win2, "推送完成", &format!("{n_done} 推送成功"));
                    }
                    crate::rt::StreamMsg::Done(Err(e)) => {
                        if cancelled {
                            dialogs::info(&win2, "已取消", &format!("已中断推送 {n_done}。"));
                        } else {
                            dialogs::show_err(&win2, &e);
                        }
                    }
                }
                reload(&st2);
            },
        );
    });

    // 删除
    let pop_x = pop.clone();
    let win_x = win.clone();
    let n_x = name.clone();
    let st_x = Rc::clone(st);
    btn_del.connect_clicked(move |_| {
        pop_x.popdown();
        dialogs::confirm(&win_x, "删除模型", &format!("确定删除模型 {n_x} ？此操作不可恢复。"), "删除", {
            let win = win_x.clone();
            let n = n_x.clone();
            let st = Rc::clone(&st_x);
            move || {
                let win2 = win.clone();
                let client = ollama::client();
                let n2 = n.clone();
                let st2 = Rc::clone(&st);
                rt::spawn(
                    async move { ollama::models::delete_model(&client, &n2).await },
                    move |res| {
                        match res {
                            Ok(()) => dialogs::info(&win2, "已删除", "模型已删除"),
                            Err(e) => dialogs::show_err(&win2, &e.to_string()),
                        }
                        reload(&st2);
                    },
                );
            }
        });
    });
}

fn show_detail(win: &gtk4::Window, name: &str) {
    let win = win.clone();
    let n = name.to_string();
    // 必须用 ollama::client()：它读取用户配置的 base_url。
    // 此前用 OllamaClient::default() 会固定打到 localhost:11434，
    // 用户在设置里改了服务地址后，详情依旧请求默认地址（与其余功能不一致）。
    let client = ollama::client();
    rt::spawn(
        async move {
            ollama::models::show_model(&client, &n).await.map(|d| {
                // 覆盖 /api/show 返回的全部字段（含 license，此前被遗漏）
                let mut s = String::new();
                s.push_str(&format!("模型：{n}\n"));
                if !d.family.is_empty() {
                    s.push_str(&format!("家族：{}\n", d.family));
                }
                if !d.parameter_size.is_empty() {
                    s.push_str(&format!("参数量：{}\n", d.parameter_size));
                }
                if !d.quantization.is_empty() {
                    s.push_str(&format!("量化：{}\n", d.quantization));
                }
                if !d.system.is_empty() {
                    s.push_str(&format!("\nSYSTEM：\n{}\n", d.system));
                }
                if !d.parameters.is_empty() {
                    s.push_str(&format!("\n参数：\n{}\n", d.parameters));
                }
                if !d.template.is_empty() {
                    s.push_str(&format!("\n模板：\n{}\n", d.template));
                }
                if !d.modelfile.is_empty() {
                    s.push_str(&format!("\nModelfile：\n{}\n", d.modelfile));
                }
                if !d.license.is_empty() {
                    s.push_str(&format!("\n许可证：\n{}\n", d.license));
                }
                s
            })
        },
        move |res: Result<String, _>| match res {
            // 内容长且需要复制，用可滚动的详情窗口而非信息框
            Ok(body) => dialogs::details(&win, "模型详情", &body),
            Err(e) => dialogs::show_err(&win, &e.to_string()),
        },
    );
}

/// 进度消息前缀：后台线程把进度编码进 `StreamMsg::Value` 文本，
/// 经 mpsc 送回主线程后再解析更新进度条（避免跨线程直接操作 GTK）。
const PROG_PREFIX: &str = "PROG:";

/// 解析进度文本并更新进度对话框。格式：`PROG:<pct>|<附加信息>`，
/// pct 为 -1 表示不确定（仅更新文字，保留脉冲动画）。
fn handle_progress(pd: &dialogs::ProgressDialog, s: &str) {
    if let Some(rest) = s.strip_prefix(PROG_PREFIX) {
        if let Some((pct_str, info)) = rest.split_once('|') {
            let pct: f64 = pct_str.trim().parse().unwrap_or(-1.0);
            if pct >= 0.0 {
                pd.set_fraction(pct / 100.0);
                pd.set_text(&format!("{}  {:.0}%", info, pct));
            } else {
                pd.set_text(info);
            }
        }
    }
}

/// 创建流程：进行中对话框 + 后台 create 到完成 + 刷新
pub(crate) fn create_flow(st: &Rc<Sidebar>, name: &str, modelfile: &str) {
    let win = st.win.clone();
    let st = Rc::clone(st);
    let name = name.to_string();
    let mf = modelfile.to_string();
    let pd = dialogs::progress(&win, "创建模型", false);
    pd.set_text(&format!("正在创建 {name}…（完成后自动关闭）"));

    let client = ollama::client();
    let pd2 = Rc::clone(&pd);
    let pd_done = Rc::clone(&pd);
    let st2 = Rc::clone(&st);
    let name_done = name.clone();
    rt::spawn_stream(
        move |tx| {
            crate::rt::block_on(async move {
                let res = ollama::create::create_model(&client, &name, &mf, |pct, status| {
                    let txt = if pct >= 0.0 {
                        format!("{}{}|{}", PROG_PREFIX, pct, status)
                    } else {
                        format!("{}|创建中…", PROG_PREFIX)
                        };
                        let _ = tx.send(crate::rt::StreamMsg::Value(txt));
                        }).await;
                        let _ = tx.send(crate::rt::StreamMsg::Done(
                        res.map(|_| crate::ollama::GenStats::default()).map_err(|e| e.to_string()),
                        ));
            });
        },
        move |msg| {
            if let crate::rt::StreamMsg::Value(s) = msg {
                handle_progress(&pd2, &s);
            }
        },
        move |msg: crate::rt::StreamMsg| {
            pd_done.close();
            let win = st2.win.clone();
            match msg {
                crate::rt::StreamMsg::Value(_) => {}
                crate::rt::StreamMsg::Think(_) => {}
                crate::rt::StreamMsg::Done(Ok(_)) => {
                    dialogs::info(&win, "创建完成", &format!("{name_done} 创建成功"));
                    reload(&st2);
                }
                crate::rt::StreamMsg::Done(Err(e)) => dialogs::show_err(&win, &e),
            }
        },
    );
}

/// 拉取流程：后台下载 + 非模态可取消进度框 + 完成后刷新。
/// 只关心「拉完就刷自己列表」的调用方用这个（管理页等）。
pub(crate) fn pull_flow(st: &Rc<Sidebar>, name: &str) {
    pull_flow_with(st, name, None);
}

/// 带完成回调的拉取流程。
///
/// `on_done` 在下载**终态**（成功 / 失败 / 取消）各调用一次，供云库页回填卡片状态——
/// v3.5.0 修复：此前下载完成后云库卡片不会更新，用户看到「下载完却仍显示安装」。
///
/// 行为变化（v3.5.0）：
/// - 进度框改为非模态、可取消；关闭窗口等同于取消（丢弃流即中止服务端下载）。
/// - 成功后不再弹模态信息框（下载常在数十分钟后结束，抢焦点很打扰），
///   改为桌面通知 + 列表与卡片自动刷新。
pub(crate) fn pull_flow_with(
    st: &Rc<Sidebar>,
    name: &str,
    on_done: Option<Rc<dyn Fn()>>,
) {
    let win = st.win.clone();
    let st = Rc::clone(st);
    let name = name.to_string();
    PULL_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);
    let pd = dialogs::progress(&win, "拉取模型", true);
    pd.set_text(&format!("正在拉取 {name}…（可继续使用应用）"));

    let client = ollama::client();
    let pd2 = Rc::clone(&pd);
    let pd_done = Rc::clone(&pd);
    let st2 = Rc::clone(&st);
    let name_done = name.clone();
    let cancel2 = pd.cancel_flag();
    let cb_main = on_done.clone();
    let cb_nested = on_done;
    rt::spawn_stream(
        move |tx| {
            crate::rt::block_on(async move {
                let res = ollama::pull::pull_model(&client, &name, cancel2, |pct, total, completed| {
                    let txt = if pct >= 0.0 {
                        format!("{}{}|{}/{} 字节", PROG_PREFIX, pct, completed, total)
                    } else {
                        format!("{}|正在获取清单…", PROG_PREFIX)
                    };
                    let _ = tx.send(crate::rt::StreamMsg::Value(txt));
                }).await;
                let _ = tx.send(crate::rt::StreamMsg::Done(
                    res.map(|_| crate::ollama::GenStats::default()).map_err(|e| e.to_string()),
                ));
            });
        },
        move |msg| {
            if let crate::rt::StreamMsg::Value(s) = msg {
                handle_progress(&pd2, &s);
            }
        },
        move |msg: crate::rt::StreamMsg| {
            let cancelled = pd_done.is_cancelled();
            pd_done.close();
            PULL_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
            let win = st2.win.clone();
            match msg {
                crate::rt::StreamMsg::Value(_) => {}
                crate::rt::StreamMsg::Think(_) => {}
                crate::rt::StreamMsg::Done(Ok(_)) => {
                    dialogs::notify(&win, "拉取完成", &format!("{name_done} 已拉取到本地"));
                    reload(&st2);
                    if let Some(cb) = &cb_main {
                        cb();
                    }
                }
                crate::rt::StreamMsg::Done(Err(e)) => {
                    if cancelled {
                        // 用户主动取消：不是故障，不弹错误框
                        reload(&st2);
                        if let Some(cb) = &cb_main {
                            cb();
                        }
                        return;
                    }
                    // 流末尾被对端切断时，模型可能已实际落盘；二次校验 /api/tags
                    // 若已存在则按成功处理，避免用户看到「下载完却报错」。
                    let st3 = Rc::clone(&st2);
                    let win3 = win.clone();
                    let name3 = name_done.clone();
                    let err = e;
                    let cb3 = cb_nested.clone();
                    let st_retry = Rc::clone(&st2);
                    rt::spawn(
                        async move {
                            let client = ollama::client();
                            ollama::models::list_models(&client).await.map_err(|e| e.to_string())
                        },
                        move |res: Result<Vec<ModelInfo>, String>| {
                            let found = res.map(|ms| {
                                ms.iter().any(|m| m.name == name3)
                            }).unwrap_or(false);
                            if found {
                                dialogs::notify(&win3, "拉取完成", &format!("{name3} 已拉取到本地"));
                                reload(&st3);
                            } else {
                                // v3.6.2：残留分片导致 Ollama 直接断流时，流内只有 error="EOF"。
                                // 这类失败可自愈——给出「清理残留并重试」入口，而不是一句干巴巴的报错。
                                let win_r = win3.clone();
                                let name_r = name3.clone();
                                let err_r = err.clone();
                                let st_r = Rc::clone(&st_retry);
                                dialogs::pull_failed(&win3, &name3, &err, move || {
                                    match crate::ollama::clean_partial_blobs() {
                                        Ok((n, freed)) => {
                                            if n > 0 {
                                                dialogs::notify(
                                                    &win_r,
                                                    "已清理残留分片",
                                                    &format!(
                                                        "移除 {n} 个未完成分片（释放 {}），正在重新下载 {name_r}",
                                                        crate::ollama::human_size(freed)
                                                    ),
                                                );
                                            }
                                            pull_flow(&st_r, &name_r);
                                        }
                                        Err(msg) => {
                                            // 残留多为服务账户创建，普通用户删不掉 → 提权清理
                                            let win_p = win_r.clone();
                                            let name_p = name_r.clone();
                                            let st_p = Rc::clone(&st_r);
                                            crate::engine::clean_partials_privileged(move || {
                                                let _ = msg;
                                                pull_flow(&st_p, &name_p);
                                                let _ = &win_p;
                                            });
                                        }
                                    }
                                });
                                reload(&st3);
                            }
                            if let Some(cb) = &cb3 {
                                cb();
                            }
                        },
                    );
                }
            }
        },
    );
}
