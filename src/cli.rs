//! 命令行接口。

use crate::protocol::InputFormat;
use crate::render::layout::Align;

// 未启用 overlay 特性时，PositionArg 只有测试在用，这个 re-export 本身会被判为未使用。
#[cfg_attr(not(feature = "overlay"), allow(unused_imports))]
pub use super::cli_types::{AlignArg, Cli, FormatArg, PositionArg};

impl From<FormatArg> for InputFormat {
    fn from(value: FormatArg) -> Self {
        match value {
            FormatArg::Plain => InputFormat::Plain,
            FormatArg::Jsonl => InputFormat::Jsonl,
        }
    }
}

impl From<AlignArg> for Align {
    fn from(value: AlignArg) -> Self {
        match value {
            AlignArg::Left => Align::Left,
            AlignArg::Center => Align::Center,
        }
    }
}

impl Cli {
    /// 是否启用颜色与文字属性。
    pub fn color_enabled(&self) -> bool {
        if self.no_color {
            return false;
        }
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn parses_defaults() {
        let cli = Cli::try_parse_from(["subview"]).expect("默认参数应能解析");
        assert_eq!(cli.format, FormatArg::Plain);
        assert_eq!(cli.lines, 2);
        assert_eq!(cli.align, AlignArg::Left);
        assert_eq!(cli.linger, 0.0);
        assert!(!cli.vertical);
        assert_eq!(cli.size, None);
        assert_eq!(cli.position, PositionArg::Start);
        assert!(!cli.raw && !cli.tui);
    }

    #[test]
    fn parses_background_alpha() {
        let cli = Cli::try_parse_from(["subview"]).expect("默认参数应能解析");
        assert_eq!(cli.background_alpha, 30);

        for value in ["0", "255"] {
            let cli = Cli::try_parse_from(["subview", "--background-alpha", value])
                .expect("不透明度应能解析");
            assert_eq!(cli.background_alpha, value.parse::<u8>().unwrap());
        }
        assert!(Cli::try_parse_from(["subview", "--background-alpha", "256"]).is_err());
    }

    #[test]
    fn parses_position_for_both_orientations() {
        let cli = Cli::try_parse_from(["subview", "--position", "end"]).expect("位置参数应能解析");
        assert_eq!(cli.position, PositionArg::End);

        let cli = Cli::try_parse_from(["subview", "--vertical", "--position", "end"])
            .expect("竖版位置参数应能解析");
        assert_eq!(cli.position, PositionArg::End);
    }

    #[test]
    fn rejects_old_position_names() {
        assert!(Cli::try_parse_from(["subview", "--position", "bottom"]).is_err());
        assert!(Cli::try_parse_from(["subview", "--position", "top"]).is_err());
    }

    #[test]
    fn parses_size_for_every_backend() {
        let cli = Cli::try_parse_from(["subview", "--size", "800"]).expect("尺寸参数应能解析");
        assert_eq!(cli.size, Some(800));

        let cli = Cli::try_parse_from(["subview", "--vertical", "--size", "800"])
            .expect("竖版尺寸参数应能解析");
        assert!(cli.vertical);
        assert_eq!(cli.size, Some(800));
    }

    #[test]
    fn vertical_conflicts_with_terminal_modes() {
        assert!(Cli::try_parse_from(["subview", "--vertical", "--tui"]).is_err());
        assert!(Cli::try_parse_from(["subview", "--vertical", "--raw"]).is_err());
    }

    #[test]
    fn rejects_zero_lines() {
        assert!(Cli::try_parse_from(["subview", "--lines", "0"]).is_err());
    }

    #[test]
    fn rejects_negative_linger() {
        assert!(Cli::try_parse_from(["subview", "--linger", "-1"]).is_err());
    }

    #[test]
    fn raw_and_tui_conflict() {
        assert!(Cli::try_parse_from(["subview", "--raw", "--tui"]).is_err());
    }
}
