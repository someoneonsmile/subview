//! 信号处理。
//!
//! - `SIGPIPE` 恢复默认行为：`subview | head` 时按 Unix 惯例安静退出。
//! - `SIGINT` / `SIGTERM` 只设置原子标志，由主循环负责恢复终端并退出。
//!
//! 信号处理器里不做任何非 async-signal-safe 的事情（不分配、不加锁、不写日志）。

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use crate::error::ExitCode;

static STOP: AtomicBool = AtomicBool::new(false);
static SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_signal(signal: libc::c_int) {
    SIGNAL.store(signal, Ordering::SeqCst);
    STOP.store(true, Ordering::SeqCst);
}

/// 安装信号处理器。必须在创建读线程之前调用。
pub fn install() {
    // Rust 默认忽略 SIGPIPE，这会让管道提前关闭时得到 EPIPE 错误而不是干净退出。
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
}

/// 是否收到了退出信号。
pub fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// 收到的信号编号（0 表示没有）。
pub fn signal_number() -> i32 {
    SIGNAL.load(Ordering::SeqCst)
}

/// 由信号导致的进程退出码（128 + 信号编号）；没有收到信号时返回 [`ExitCode::Ok`]。
pub fn exit_code() -> ExitCode {
    match signal_number() {
        0 => ExitCode::Ok,
        signal => ExitCode::Signal(signal),
    }
}
