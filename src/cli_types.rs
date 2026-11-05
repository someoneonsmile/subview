// ⚠️ 此文件被 build.rs 通过 include! 引入，禁止添加 use crate:: 依赖。
// 如需 crate 级别引用，请在 cli.rs 中添加。

use clap::{Parser, ValueEnum};

/// 输入格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    Plain,
    Jsonl,
}

/// 水平对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AlignArg {
    Left,
    Center,
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

    /// 显示的字幕行数（终端模式取 min(终端高度, N)）
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
