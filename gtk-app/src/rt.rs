//! GTK 主循环 与 tokio 异步的桥接
//!
//! v1.0.4 架构变更（修复「未响应」卡死）：
//! 旧实现用「主线程递归 10ms/5ms timeout」轮询 mpsc 通道取结果。
//! 当后台任务挂起时（Ollama 忙、网络慢），每个任务都会在 GLib 主循环里
//! 遗留一个 100Hz 空转定时器；轮询类任务每 3 秒又新开一个，定时器越积越多，
//! 最终饱和主循环 → 应用「未响应」→ 托盘/退出全部失效。
//!
//! 新实现：后台线程完成后经 tokio oneshot/unbounded channel 投递，
//! 主线程用 `glib::spawn_future_local` 异步等待（基于 waker 唤醒，事件驱动）
//! —— 零定时器、零轮询，主循环空闲时不再有任何周期性唤醒。

use glib::spawn_future_local;
use std::future::Future;
use std::sync::OnceLock;

// 全局 tokio runtime（多线程）
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("构建 tokio runtime 失败")
    })
}

/// 同步阻塞执行一个异步任务（仅限后台线程内部使用，严禁在 GTK 主线程调用）。
pub fn block_on<F, R>(fut: F) -> R
where
    F: Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    runtime().block_on(fut)
}

/// 在后台执行异步任务 `fut`，完成后在 GTK 主线程调用 `on_done`。
pub fn spawn<F, R, Cb>(fut: F, on_done: Cb)
where
    F: Future<Output = R> + Send + 'static,
    R: Send + 'static,
    Cb: FnOnce(R) + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel::<R>();

    // 后台线程：执行异步任务，结果经 oneshot 送回主线程
    std::thread::spawn(move || {
        let _ = tx.send(block_on(fut));
    });

    spawn_future_local(async move {
        if let Ok(val) = rx.await {
            on_done(val);
        }
    });
}

/// 流式：后台线程执行异步闭包 `fut`（其中可调用 `send` 发 token 或结束信号），
/// 主线程每收到 token 调 `on_token` 更新 UI，收到 `Done(..)` 调 `on_done` 后停止。
/// `on_token` / `on_done` 只会在 GTK 主线程被调用，因此不需要 `Send`（可捕获 `Rc` 更新 UI）。
pub fn spawn_stream<F, CT, CD>(
    fut: F,
    on_token: CT,
    on_done: CD,
) where
    F: FnOnce(tokio::sync::mpsc::UnboundedSender<StreamMsg>) + Send + 'static,
    CT: Fn(StreamMsg) + 'static,
    CD: Fn(StreamMsg) + 'static,
{
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamMsg>();

    // 后台线程：执行真正的异步工作（tx.send 为同步非阻塞，与旧 std sender 用法一致）
    std::thread::spawn(move || {
        fut(tx);
    });

    spawn_future_local(async move {
        while let Some(msg) = rx.recv().await {
            match msg {
                StreamMsg::Done(_) => {
                    on_done(msg);
                    break;
                }
                other => on_token(other),
            }
        }
    });
}

/// 流式消息：Value 携带一个 token 片段；Think 携带思考过程片段；Done 携带最终结果。
/// Ok 附带本次生成的统计信息（token 数、速度、耗时），Err 为错误描述。
#[derive(Clone, Debug)]
pub enum StreamMsg {
    Value(String),
    Think(String),
    Done(Result<crate::ollama::GenStats, String>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_msg_is_debug_and_clone() {
        let m = StreamMsg::Value("hi".into());
        let c = m.clone();
        assert!(format!("{c:?}").contains("hi"));
    }

    #[test]
    fn block_on_runs_future() {
        let v = block_on(async { 1 + 1 });
        assert_eq!(v, 2);
    }
}
