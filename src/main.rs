//! subview：把标准输入的字幕流实时显示到屏幕。
//!
//! 设计见 `DESIGN.md`，协议见 `docs/protocol.md`。

mod app;
mod cli;
mod cli_types;
mod config;
mod error;
mod protocol;
mod reader;
mod render;
mod run;
mod signal;
mod state;
mod wrap;

use std::io::{self, IsTerminal};

use clap::Parser as _;

use app::{Capabilities, Mode};
use cli::Cli;
#[cfg(feature = "overlay")]
use config::OverlayConfig;
use config::{ReaderOptions, TextPolicy};
use error::StartupError;

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

    let caps = Capabilities {
        overlay: cfg!(feature = "overlay"),
    };
    // 无悬浮窗能力且走默认模式时，进入终端界面前先说明回退原因。
    let fallback_hint = !caps.overlay && !cli.raw && !cli.tui && !cli.vertical;
    let mode = match app::resolve(
        &cli,
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
        caps,
    ) {
        Ok(mode) => {
            if fallback_hint && mode == Mode::Tui {
                eprintln!(
                    "subview: 该二进制未编译悬浮窗支持（--no-default-features），回退到终端界面"
                );
            }
            mode
        }
        Err(error) => {
            if fallback_hint && error == StartupError::TuiNeedsTty {
                eprintln!(
                    "subview: 该二进制未编译悬浮窗支持（--no-default-features），回退到终端界面"
                );
            }
            return app::report(&error);
        }
    };

    let reader_options = ReaderOptions::from(&cli);

    if cli.debug {
        eprintln!(
            "subview: mode={} format={:?} lines={} size={:?} align={:?} linger={} color={}",
            mode_label(mode, &cli),
            reader_options.format,
            cli.lines,
            cli.size,
            cli.align,
            cli.linger,
            !cli.no_color
        );
    }

    match mode {
        Mode::PlainFilter => run::filter::run(ReaderOptions {
            policy: TextPolicy::PASSTHROUGH,
            ..reader_options
        }),
        Mode::Tui => run::terminal::run_tui(&cli, reader_options),
        Mode::RawTty => run::terminal::run_raw(&cli, reader_options),
        Mode::Overlay => run_overlay(&cli, reader_options),
    }
}

fn mode_label(mode: Mode, cli: &Cli) -> &'static str {
    match mode {
        Mode::Overlay if cli.vertical => "overlay-vertical",
        Mode::Overlay => "overlay",
        Mode::Tui => "tui",
        Mode::RawTty => "raw",
        Mode::PlainFilter => "filter",
    }
}

/// 默认模式：尝试桌面悬浮窗，不可用时按规则回退。
fn run_overlay(cli: &Cli, options: ReaderOptions) -> i32 {
    #[cfg(feature = "overlay")]
    {
        let config = OverlayConfig::from(cli);
        match render::overlay::run(&config, options) {
            render::overlay::Outcome::Exited(code) => code,
            render::overlay::Outcome::Unavailable(reason) => {
                // 竖版只有悬浮窗后端支持，不能静默回退成横版终端界面。
                if cli.vertical {
                    eprintln!("subview: 竖版界面需要桌面悬浮窗（X11）：{reason}");
                    return error::ExitCode::Runtime.as_i32();
                }
                eprintln!("subview: {reason}");
                eprintln!("subview: 回退到终端界面（可用 --tui 显式指定）");
                if !io::stdout().is_terminal() {
                    return app::report(&StartupError::TuiNeedsTty);
                }
                run::terminal::run_tui(cli, options)
            }
        }
    }
    #[cfg(not(feature = "overlay"))]
    {
        // `app::resolve` 在无悬浮窗能力时不会返回 `Mode::Overlay`。
        let _ = (cli, options);
        unreachable!("无悬浮窗能力时不应进入悬浮窗分支")
    }
}
