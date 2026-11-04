//! 命令行接口。

use clap::{Parser, ValueEnum};

use crate::protocol::InputFormat;
use crate::render::layout::Align;

/// 输入格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    Plain,
    Jsonl,
}

impl From<FormatArg> for InputFormat {
    fn from(value: FormatArg) -> Self {
        match value {
            FormatArg::Plain => InputFormat::Plain,
            FormatArg::Jsonl => InputFormat::Jsonl,
        }
    }
}

/// 水平对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AlignArg {
    Left,
    Center,
}

impl From<AlignArg> for Align {
    fn from(value: AlignArg) -> Self {
        match value {
            AlignArg::Left => Align::Left,
            AlignArg::Center => Align::Center,
        }
    }
}

/// 悬浮窗位置。
///
/// 取值是中性的 `start` / `end`，实际方位随布局：横版沿垂直轴
/// （start = 贴底、end = 贴顶），竖版沿水平轴（start = 贴右、end = 贴左）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PositionArg {
    Start,
    End,
}

fn parse_linger(value: &str) -> Result<f64, String> {
    let seconds: f64 = value.parse().map_err(|_| format!("无效的秒数: {value}"))?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err("必须是非负的秒数".to_string());
    }
    Ok(seconds)
}

/// 把标准输入的字幕流实时显示到屏幕。
#[derive(Debug, Parser)]
#[command(name = "subview", version, about, long_about = None)]
pub struct Cli {
    /// 输入格式
    #[arg(long, value_enum, default_value_t = FormatArg::Plain, value_name = "FORMAT")]
    pub format: FormatArg,

    /// 保留最近几条字幕
    #[arg(
        long,
        default_value_t = 2,
        value_name = "N",
        value_parser = clap::value_parser!(u16).range(1..)
    )]
    pub lines: u16,

    /// 折行宽度（终端，列）或窗口尺寸（悬浮窗，像素：横版为宽、竖版为高）
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u16).range(4..)
    )]
    pub size: Option<u16>,

    /// 水平对齐
    #[arg(long, value_enum, default_value_t = AlignArg::Left, value_name = "ALIGN")]
    pub align: AlignArg,

    /// 最后一条字幕驻留多少秒后淡出（0 = 常驻）
    #[arg(long, default_value_t = 0.0, value_name = "SEC", value_parser = parse_linger)]
    pub linger: f64,

    /// 关闭颜色与文字属性
    #[arg(long)]
    pub no_color: bool,

    /// 悬浮窗背景条不透明度（0–255，0 = 完全透明；仅悬浮窗）
    #[arg(long, default_value_t = 30, value_name = "N")]
    pub background_alpha: u8,

    /// 纯 ANSI 过滤器模式：不接管屏幕，原地更新
    #[arg(long, conflicts_with = "tui")]
    pub raw: bool,

    /// 强制使用终端界面（默认使用桌面悬浮窗）
    #[arg(long)]
    pub tui: bool,

    /// 悬浮窗位置：横版 start 贴底 / end 贴顶，竖版 start 贴右 / end 贴左
    #[arg(long, value_enum, default_value_t = PositionArg::Start, value_name = "POS")]
    pub position: PositionArg,

    /// 竖版悬浮窗：字从上到下、列从右到左（仅桌面悬浮窗）
    #[arg(long, conflicts_with_all = ["tui", "raw"])]
    pub vertical: bool,

    /// 打印调试信息到 stderr
    #[arg(long)]
    pub debug: bool,
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
