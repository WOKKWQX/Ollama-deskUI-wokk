//! 卡死黑匣子（v1.0.13 灵敏化 → v1.0.14 现场抓捕 + 自动止损）
//!
//! 背景：应用反复出现「未响应」级卡死（托盘失效、退不出），现场无 panic 痕迹。
//! v1.0.12 版阈值（6 秒）过钝——实测卡死多在 6 秒内被用户杀掉，一条都没抓到。
//!
//! 实机 watchdog.log 关键证据（v1.0.12 抓到 4 次）：
//! 卡死时主线程 `syscall=running`——不在任何系统调用上阻塞，
//! 而是**用户态 CPU 空转**把界面主循环饿死。wchan/syscall 只能看到这一层，
//! 要定罪必须拿**用户态 Rust 调用栈**。
//!
//! v1.0.14 机制（灵敏感知 + 现场抓捕 + 自动止损）：
//! - 主线程每 500ms 写一次心跳；
//! - 看门狗每 1 秒检查：超过 2.5 秒无心跳即判定卡死并立即写现场；
//! - 卡死开始/持续超 30 秒时，经 `tgkill` 向进程内**所有线程**发 SIGUSR2，
//!   信号处理器在各自线程内抓取自己的 Rust 调用栈并 eprintln
//!   （落到 ~/.xsession-errors）；
//! - 连续卡死 >30 秒：补抓栈后 `_exit(70)` 自动退出应用——托盘随即释放，
//!   用户**无需重启系统**；
//! - 恢复时记录「主循环恢复 + 本次卡死总时长」；
//! - 全部写入 `~/.cache/offideskollama/watchdog.log` 并 eprintln。
//!
//! 诚实边界：信号处理器内 `Backtrace::new()`（含堆分配）并非严格
//! async-signal-safe，但已证实卡死场景是 CPU 空转而非 malloc 中途死锁，
//! 实践可行；若处理器自身异常，glibc 终止进程且已输出日志仍在。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// 主线程心跳（Unix 毫秒时间戳，每 500ms 盖章）
pub static HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);
static HANG_ACTIVE: AtomicBool = AtomicBool::new(false);
static HANG_START_MS: AtomicU64 = AtomicU64::new(0);
static LAST_REPORT_MS: AtomicU64 = AtomicU64::new(0);

const HANG_THRESHOLD_MS: u64 = 2500;
const PROGRESS_INTERVAL_MS: u64 = 5000;
/// 持续卡死超过该时长自动退出应用（免重启系统）
const EXIT_THRESHOLD_MS: u64 = 30_000;

static LAST_CPU_TICKS: AtomicU64 = AtomicU64::new(0);

/// 主线程累计 CPU 时间（utime+stime，单位 tick；CLK_TCK 通常=100）
fn main_cpu_ticks() -> u64 {
    let p = format!("/proc/self/task/{}/stat", std::process::id());
    let Ok(s) = std::fs::read_to_string(p) else { return 0 };
    // comm 字段可能含空格，从最后一个 ')' 之后切分
    let Some((_, rest)) = s.rsplit_once(')') else { return 0 };
    let f: Vec<&str> = rest.split_whitespace().collect();
    // rest 自 state(第3字段) 起：utime=原第14字段 → rest 索引 11，stime=索引 12
    let utime: u64 = f.get(11).and_then(|v| v.parse().ok()).unwrap_or(0);
    let stime: u64 = f.get(12).and_then(|v| v.parse().ok()).unwrap_or(0);
    utime + stime
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

extern "C" fn on_capture_signal(_sig: libc::c_int) {
    // 在收到信号的线程内执行：抓取本线程调用栈。
    // SIGUSR2 仅由 capture_all_threads 发出，不会误触发。
    let bt = backtrace::Backtrace::new();
    let tid = unsafe { libc::gettid() };
    eprintln!("[bt] tid={tid} 栈（最上=当前执行点）:\n{bt:?}");
}

/// 在 GTK 主线程初始化时调用一次：安装心跳 + 看门狗 + 信号处理器。
pub fn start() {
    glib::timeout_add_local(std::time::Duration::from_millis(500), || {
        HEARTBEAT_MS.store(now_ms(), Ordering::SeqCst);
        glib::ControlFlow::Continue
    });

    unsafe {
        // SIGUSR2 默认行为是终止进程，必须先换成本处理器
        libc::signal(libc::SIGUSR2, on_capture_signal as libc::sighandler_t);
    }

    std::thread::Builder::new()
        .name("watchdog".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let last = HEARTBEAT_MS.load(Ordering::SeqCst);
            if last == 0 {
                continue; // 主循环还没跑起来
            }
            let now = now_ms();
            let gap = now.saturating_sub(last);

            if gap > HANG_THRESHOLD_MS {
                if !HANG_ACTIVE.load(Ordering::SeqCst) {
                    // 新的一次卡死：立即抓现场 + 抓用户态栈
                    HANG_ACTIVE.store(true, Ordering::SeqCst);
                    HANG_START_MS.store(now.saturating_sub(gap), Ordering::SeqCst);
                    LAST_REPORT_MS.store(now, Ordering::SeqCst);
                    LAST_CPU_TICKS.store(main_cpu_ticks(), Ordering::SeqCst);
                    report(&format!("卡死开始（已无心跳 {gap}ms）"), true);
                    capture_all_threads();
                } else {
                    // 进行中：间隔性追加（附主线程 CPU 增量，区分「空转」与「等待」）
                    let last_report = LAST_REPORT_MS.load(Ordering::SeqCst);
                    if now.saturating_sub(last_report) >= PROGRESS_INTERVAL_MS {
                        LAST_REPORT_MS.store(now, Ordering::SeqCst);
                        let cur = main_cpu_ticks();
                        let delta = cur.saturating_sub(LAST_CPU_TICKS.swap(cur, Ordering::SeqCst));
                        report(
                            &format!(
                                "卡死持续中（已无心跳 {gap}ms，近5s主线程CPU≈{} tick）",
                                delta
                            ),
                            false,
                        );
                    }
                    // 持续卡死超阈值：补抓栈后自动退出，释放托盘免重启
                    if gap > EXIT_THRESHOLD_MS {
                        let line = format!(
                            "主循环无心跳已 {gap}ms，超 {EXIT_THRESHOLD_MS}ms 阈值——\
                             补抓现场后自动退出应用（exit 70）以免用户重启系统。\
                             请回传 watchdog.log 与 ~/.xsession-errors 中 [bt]/[watchdog] 段。"
                        );
                        eprintln!("[watchdog] {line}");
                        append_log(&line);
                        capture_all_threads();
                        std::thread::sleep(std::time::Duration::from_millis(1500));
                        std::process::exit(70);
                    }
                }
            } else if HANG_ACTIVE.swap(false, Ordering::SeqCst) {
                // 恢复了
                let start = HANG_START_MS.load(Ordering::SeqCst);
                let total = now.saturating_sub(start);
                let line = format!("主循环恢复：本次卡死约 {total}ms 后自行恢复（用户未杀进程）");
                eprintln!("[watchdog] {line}");
                append_log(&line);
            }
        })
        .expect("启动 watchdog 线程失败");
}

/// 向进程内所有线程发 SIGUSR2，让各线程自抓 Rust 调用栈
fn capture_all_threads() {
    let pid = std::process::id() as i32;
    if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
        for t in tasks.flatten() {
            let tid: i32 = match t.file_name().to_string_lossy().parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            unsafe {
                libc::syscall(libc::SYS_tgkill, pid, tid, libc::SIGUSR2);
            }
        }
    }
    // 给各线程的信号处理器留输出时间
    std::thread::sleep(std::time::Duration::from_millis(500));
}

fn report(reason: &str, with_threads: bool) {
    let body = if with_threads {
        dump_threads()
    } else {
        String::new()
    };
    let line = format!("{reason}{body}");
    eprintln!("[watchdog] {line}");
    append_log(&line);
}

/// 抓取当前进程所有线程的阻塞现场
pub fn dump_threads() -> String {
    let mut out = String::from(" —— 线程现场：");
    let tasks = match std::fs::read_dir("/proc/self/task") {
        Ok(d) => d,
        Err(e) => return format!("（读 /proc/self/task 失败：{e}）"),
    };
    for t in tasks.flatten() {
        let tid = t.file_name().to_string_lossy().to_string();
        let comm = std::fs::read_to_string(t.path().join("comm"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let wchan = std::fs::read_to_string(t.path().join("wchan"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let syscall = std::fs::read_to_string(t.path().join("syscall"))
            .unwrap_or_default()
            .split_whitespace()
            .next()
            .unwrap_or("-")
            .to_string();
        out.push_str(&format!(
            "\n  tid {tid} [{comm}] wchan={wchan} syscall={syscall}"
        ));
    }
    out
}

fn append_log(report: &str) {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = std::path::Path::new(&home).join(".cache").join("offideskollama");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("watchdog.log"))
    {
        use std::io::Write;
        let _ = f.write_all(format!("[{ts}] {report}\n\n").as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_ms_is_monotonic_enough() {
        let a = now_ms();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = now_ms();
        assert!(b >= a);
    }

    #[test]
    fn dump_threads_lists_live_threads() {
        let s = dump_threads();
        assert!(s.contains("tid "), "至少一条线程行: {s}");
        assert!(s.contains("wchan="), "包含内核阻塞点: {s}");
    }
}
