//! 运行模式解析：把 CLI、终端状态与编译期能力解析成唯一的运行模式。
//!
//! 这是“当前该用哪个后端”的单一事实来源；`main` 只负责按结果分发。

use crate::cli::Cli;
use crate::error::StartupError;

/// 运行模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 桌面悬浮窗（运行期不可用时由入口回退）。
    Overlay,
    /// 终端界面（备用屏幕）。
    Tui,
    /// 不接管屏幕，原地重绘。
    RawTty,
    /// 纯文本过滤器：只输出定稿行。
    PlainFilter,
}

/// 编译期能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// 是否编译了悬浮窗支持。
    pub overlay: bool,
}

/// 解析运行模式。
///
/// 决策顺序：stdin 必须是管道 → `--raw` → `--tui` → 竖版能力 → 默认后端。
pub fn resolve(
    cli: &Cli,
    stdin_tty: bool,
    stdout_tty: bool,
    caps: Capabilities,
) -> Result<Mode, StartupError> {
    if stdin_tty {
        return Err(StartupError::StdinIsTerminal);
    }
    if cli.raw {
        return Ok(if stdout_tty {
            Mode::RawTty
        } else {
            Mode::PlainFilter
        });
    }
    if cli.tui {
        return if stdout_tty {
            Ok(Mode::Tui)
        } else {
            Err(StartupError::TuiNeedsTty)
        };
    }
    if !caps.overlay && cli.vertical {
        return Err(StartupError::VerticalNeedsOverlay);
    }
    if caps.overlay {
        return Ok(Mode::Overlay);
    }
    if stdout_tty {
        Ok(Mode::Tui)
    } else {
        Err(StartupError::TuiNeedsTty)
    }
}

/// 打印启动错误并返回退出码。
pub fn report(error: &StartupError) -> i32 {
    match error {
        StartupError::StdinIsTerminal => {
            eprintln!("subview: 从标准输入读取字幕流，而不是从终端。");
            eprintln!("示例：");
            eprintln!("    producer | subview");
            eprintln!("    printf '你好\\n' | subview");
        }
        StartupError::TuiNeedsTty => {
            eprintln!("subview: 终端界面需要终端输出；输出到管道或文件时请使用 --raw。");
            eprintln!("示例：");
            eprintln!("    producer | subview --raw > captions.log");
        }
        StartupError::VerticalNeedsOverlay => {
            eprintln!(
                "subview: 竖版界面需要桌面悬浮窗；该二进制未编译悬浮窗支持（--no-default-features）"
            );
        }
    }
    error.exit_code().as_i32()
}

#[cfg(test)]
mod tests {
    use super::*;

    use clap::Parser as _;

    fn cli(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("参数应能解析")
    }

    fn caps(overlay: bool) -> Capabilities {
        Capabilities { overlay }
    }

    #[test]
    fn stdin_tty_is_rejected() {
        for args in [
            &["subview"][..],
            &["subview", "--raw"],
            &["subview", "--tui"],
            &["subview", "--vertical"],
        ] {
            assert_eq!(
                resolve(&cli(args), true, true, caps(true)),
                Err(StartupError::StdinIsTerminal)
            );
        }
    }

    #[test]
    fn raw_depends_on_stdout_tty() {
        assert_eq!(
            resolve(&cli(&["subview", "--raw"]), false, true, caps(true)),
            Ok(Mode::RawTty)
        );
        assert_eq!(
            resolve(&cli(&["subview", "--raw"]), false, false, caps(true)),
            Ok(Mode::PlainFilter)
        );
    }

    #[test]
    fn tui_requires_stdout_tty() {
        assert_eq!(
            resolve(&cli(&["subview", "--tui"]), false, true, caps(true)),
            Ok(Mode::Tui)
        );
        assert_eq!(
            resolve(&cli(&["subview", "--tui"]), false, false, caps(true)),
            Err(StartupError::TuiNeedsTty)
        );
    }

    #[test]
    fn default_mode_prefers_overlay_when_available() {
        assert_eq!(
            resolve(&cli(&["subview"]), false, false, caps(true)),
            Ok(Mode::Overlay)
        );
        assert_eq!(
            resolve(&cli(&["subview", "--vertical"]), false, false, caps(true)),
            Ok(Mode::Overlay)
        );
    }

    #[test]
    fn default_mode_falls_back_to_tui_without_overlay() {
        assert_eq!(
            resolve(&cli(&["subview"]), false, true, caps(false)),
            Ok(Mode::Tui)
        );
        assert_eq!(
            resolve(&cli(&["subview"]), false, false, caps(false)),
            Err(StartupError::TuiNeedsTty)
        );
    }

    #[test]
    fn vertical_without_overlay_is_rejected() {
        assert_eq!(
            resolve(&cli(&["subview", "--vertical"]), false, true, caps(false)),
            Err(StartupError::VerticalNeedsOverlay)
        );
    }
}
