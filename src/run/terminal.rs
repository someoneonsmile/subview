//! 终端后端装配：备用屏幕（`--tui`）与原地重绘（`--raw` 且输出到终端）。

use std::io;

use crate::cli::Cli;
use crate::config::ReaderOptions;
use crate::error::ExitCode;
use crate::render::layout::Align;
use crate::render::raw::RawRenderer;
use crate::render::tui::TuiRenderer;

/// 屏幕接管方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    /// 切换备用屏幕（`--tui`）。
    Alternate,
    /// 不接管屏幕，原地重绘（`--raw` 且 stdout 是终端）。
    Inline,
}

/// 终端界面（备用屏幕）。
pub fn run_tui(cli: &Cli, options: ReaderOptions) -> ExitCode {
    run(cli, options, Screen::Alternate)
}

/// 原地重绘（`--raw` 且输出到终端）。
pub fn run_raw(cli: &Cli, options: ReaderOptions) -> ExitCode {
    run(cli, options, Screen::Inline)
}

fn run(cli: &Cli, options: ReaderOptions, screen: Screen) -> ExitCode {
    match stream_loop(cli, options, screen) {
        Ok(()) => crate::signal::exit_code(),
        Err(error) => {
            crate::ui::note(error);
            ExitCode::Runtime
        }
    }
}

fn stream_loop(cli: &Cli, options: ReaderOptions, screen: Screen) -> io::Result<()> {
    let align = Align::from(cli.align);
    let color = cli.color_enabled();
    let width_limit = cli.size.map(usize::from);
    let stdout = io::stdout();

    match screen {
        Screen::Alternate => {
            let mut renderer =
                TuiRenderer::new(stdout.lock(), align, width_limit, color, cli.lines as usize);
            renderer.enter()?;
            let result = drive_session(&mut renderer, options, cli.linger);
            renderer.exit()?;
            result
        }
        Screen::Inline => {
            let mut renderer =
                RawRenderer::new(stdout.lock(), align, width_limit, color, cli.lines as usize);
            let result = drive_session(&mut renderer, options, cli.linger);
            renderer.finish()?;
            result
        }
    }
}

fn drive_session<R: crate::render::Renderer>(
    renderer: &mut R,
    options: ReaderOptions,
    linger: f64,
) -> io::Result<()> {
    let input = crate::run::spawn_input(options);
    crate::run::drive(renderer, &input.state, &input.dirty, &input.eof, linger)
}
