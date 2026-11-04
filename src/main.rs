//! subview：把标准输入的字幕流实时显示到屏幕。
//!
//! 设计见 `DESIGN.md`，协议见 `docs/protocol.md`。

mod cli;
mod protocol;
mod reader;
mod render;
mod signal;
mod state;
mod wrap;

use std::io::{self, IsTerminal, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser as _;

use cli::Cli;
use protocol::{Event, InputFormat, Parser as ProtocolParser};
use render::Backend;
use render::layout::Align;
use render::raw::RawRenderer;
use render::tui::TuiRenderer;
use state::CaptionState;

/// linger 触发后的淡出时长（秒）。
const FADE_SECS: f64 = 0.8;
/// 主循环轮询间隔。
const TICK: Duration = Duration::from_millis(16);

fn main() {
    let code = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).unwrap_or_else(|_| {
        eprintln!("subview: 内部错误，已中止");
        1
    });
    std::process::exit(code);
}

fn run() -> i32 {
    let cli = Cli::parse();
    signal::install();

    if io::stdin().is_terminal() {
        eprintln!("subview: 从标准输入读取字幕流，而不是从终端。");
        eprintln!("示例：");
        eprintln!("    producer | subview");
        eprintln!("    printf '你好\\n' | subview");
        return 2;
    }

    let format = InputFormat::from(cli.format);
    let stdout_is_tty = io::stdout().is_terminal();

    if cli.debug {
        let mode = if cli.raw && stdout_is_tty {
            "raw"
        } else if cli.raw {
            "filter"
        } else if cli.tui {
            "tui"
        } else {
            "overlay"
        };
        eprintln!(
            "subview: mode={mode} format={:?} lines={} width={:?} align={:?} linger={} color={}",
            format, cli.lines, cli.width, cli.align, cli.linger, !cli.no_color
        );
    }

    if cli.raw {
        return if stdout_is_tty {
            run_stream(&cli, format, true, StreamMode::RawTty)
        } else {
            // 纯文本过滤器：原样透传，不做显示层处理。
            run_plain_text(format, false)
        };
    }
    if cli.tui {
        return run_terminal(&cli, format, stdout_is_tty);
    }

    // 默认：桌面悬浮窗（图形环境不可用时回退到终端界面）。
    #[cfg(feature = "overlay")]
    {
        match render::overlay::run(&cli, format, true) {
            render::overlay::Outcome::Exited(code) => return code,
            render::overlay::Outcome::Unavailable(reason) => {
                eprintln!("subview: {reason}");
                eprintln!("subview: 回退到终端界面（可用 --tui 显式指定）");
            }
        }
    }
    #[cfg(not(feature = "overlay"))]
    {
        eprintln!("subview: 该二进制未编译悬浮窗支持（--no-default-features），回退到终端界面");
    }
    run_terminal(&cli, format, stdout_is_tty)
}

/// 终端界面（需要标准输出是终端）。
fn run_terminal(cli: &Cli, format: InputFormat, stdout_is_tty: bool) -> i32 {
    if !stdout_is_tty {
        eprintln!("subview: 终端界面需要终端输出；输出到管道或文件时请使用 --raw。");
        eprintln!("示例：");
        eprintln!("    producer | subview --raw > captions.log");
        return 2;
    }
    run_stream(cli, format, true, StreamMode::Tui)
}

/// 屏幕模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamMode {
    /// 接管终端，底部固定区域显示。
    Tui,
    /// 不接管终端，原地重绘。
    RawTty,
}

fn run_stream(cli: &Cli, format: InputFormat, strip_control: bool, mode: StreamMode) -> i32 {
    match stream_loop(cli, format, strip_control, mode) {
        Ok(()) => signal::exit_code(),
        Err(error) => {
            eprintln!("subview: {error}");
            1
        }
    }
}

fn stream_loop(
    cli: &Cli,
    format: InputFormat,
    strip_control: bool,
    mode: StreamMode,
) -> io::Result<()> {
    let state = Arc::new(Mutex::new(CaptionState::new(cli.lines as usize)));
    let dirty = Arc::new(AtomicBool::new(false));
    let eof = Arc::new(AtomicBool::new(false));
    let dirty_reader = dirty.clone();
    let eof_reader = eof.clone();
    reader::spawn_reader(state.clone(), format, strip_control, move |event| {
        dirty_reader.store(true, Ordering::Release);
        if event == reader::ReaderEvent::Finished {
            eof_reader.store(true, Ordering::Release);
        }
    });

    let align = Align::from(cli.align);
    let color = cli.color_enabled();
    let width_limit = cli.width.map(usize::from);
    let stdout = io::stdout();

    match mode {
        StreamMode::Tui => {
            let mut renderer = TuiRenderer::new(stdout.lock(), align, width_limit, color);
            renderer.enter()?;
            let result = drive(&mut renderer, &state, &dirty, &eof, cli.linger);
            renderer.exit()?;
            result
        }
        StreamMode::RawTty => {
            let mut renderer = RawRenderer::new(stdout.lock(), align, width_limit, color);
            let result = drive(&mut renderer, &state, &dirty, &eof, cli.linger);
            renderer.finish()?;
            result
        }
    }
}

/// 主循环：等事件、必要时重绘、处理 linger 淡出与 EOF。
fn drive<B: Backend>(
    backend: &mut B,
    state: &Arc<Mutex<CaptionState>>,
    dirty: &Arc<AtomicBool>,
    eof: &Arc<AtomicBool>,
    linger: f64,
) -> io::Result<()> {
    let linger_enabled = linger > 0.0;
    let mut last_event = Instant::now();
    let mut fade = 0u8;
    let mut linger_done = !linger_enabled;

    loop {
        if signal::stop_requested() {
            return Ok(());
        }

        let changed = dirty.swap(false, Ordering::AcqRel);
        if changed {
            last_event = Instant::now();
            fade = 0;
            linger_done = !linger_enabled;
        }

        let mut needs_redraw = changed;
        if !linger_done && last_event.elapsed().as_secs_f64() > linger {
            let elapsed = last_event.elapsed().as_secs_f64();
            let progress = ((elapsed - linger) / FADE_SECS).clamp(0.0, 1.0);
            let new_fade = (progress * 100.0).round() as u8;
            if new_fade != fade {
                fade = new_fade;
                needs_redraw = true;
            }
            if progress >= 1.0 {
                lock_state(state)?.clear_display();
                linger_done = true;
                needs_redraw = true;
            }
        }

        if needs_redraw {
            let state = lock_state(state)?;
            let (cols, rows) = terminal_size();
            backend.draw(&state, cols, rows, fade)?;
        }

        if eof.load(Ordering::Acquire) && !dirty.load(Ordering::Acquire) {
            // 退出前把最终状态渲染出来。
            let state = lock_state(state)?;
            let (cols, rows) = terminal_size();
            backend.draw(&state, cols, rows, fade)?;
            return Ok(());
        }

        thread::sleep(TICK);
    }
}

fn lock_state(state: &Arc<Mutex<CaptionState>>) -> io::Result<MutexGuard<'_, CaptionState>> {
    state
        .lock()
        .map_err(|_| io::Error::other("字幕状态锁已中毒"))
}

fn terminal_size() -> (u16, u16) {
    // pty 未设置窗口大小时会返回 0×0，这种尺寸无法渲染，退回默认值。
    match crossterm::terminal::size() {
        Ok((cols, rows)) if cols > 0 && rows > 0 => (cols, rows),
        _ => (80, 24),
    }
}

/// 非终端输出：只把定稿字幕按行写出，作为普通文本过滤器使用。
fn run_plain_text(format: InputFormat, strip_control: bool) -> i32 {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let mut parser = ProtocolParser::new(format, strip_control);
    let mut events: Vec<Event> = Vec::new();
    let mut buffer = [0u8; 8192];

    loop {
        if signal::stop_requested() {
            break;
        }
        match reader.read(&mut buffer) {
            Ok(0) => {
                events.clear();
                parser.finish(&mut events);
                write_final_lines(&mut out, &events);
                if let Err(error) = out.flush() {
                    eprintln!("subview: {error}");
                    return 1;
                }
                break;
            }
            Ok(read) => {
                events.clear();
                parser.feed(&buffer[..read], &mut events);
                write_final_lines(&mut out, &events);
                if let Err(error) = out.flush() {
                    eprintln!("subview: {error}");
                    return 1;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                eprintln!("subview: {error}");
                return 1;
            }
        }
    }

    signal::exit_code()
}

fn write_final_lines<W: Write>(out: &mut W, events: &[Event]) {
    for event in events {
        if let Event::Final(caption) = event {
            if let Some(speaker) = &caption.speaker {
                let _ = write!(out, "{}", crate::render::layout::speaker_prefix(speaker));
            }
            let _ = writeln!(out, "{}", caption.text);
        }
    }
}
