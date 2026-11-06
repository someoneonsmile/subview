//! 运行配置：CLI 解析结果翻译成渲染层与读线程使用的领域参数。
//!
//! 渲染层只依赖这里的数据结构，不依赖 clap 生成的 `Cli`。

use crate::protocol::InputFormat;
#[cfg(feature = "overlay")]
use crate::render::layout::Align;

/// 悬浮窗位置。
///
/// 中性命名：实际方位随布局解释——横版沿垂直轴（`Start` 贴底、`End` 贴顶），
/// 竖版沿水平轴（`Start` 贴右、`End` 贴左）。
#[cfg(feature = "overlay")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Start,
    End,
}

/// 文本处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPolicy {
    /// 是否剥除控制字符（防止终端转义注入）。
    pub strip_control: bool,
}

impl TextPolicy {
    /// 显示模式：剥除控制字符。
    pub const SANITIZED: Self = Self {
        strip_control: true,
    };
    /// 纯文本透传：保留原始字节（非终端输出，无注入面）。
    pub const PASSTHROUGH: Self = Self {
        strip_control: false,
    };
}

/// 悬浮窗后端配置。
#[cfg(feature = "overlay")]
#[derive(Debug, Clone, Copy)]
pub struct OverlayConfig {
    /// 显示的字幕行数；竖版表示列数。
    pub lines: u16,
    /// 尺寸参数：横版为窗口宽度（像素），竖版为窗口高度（像素）。
    pub size: Option<u16>,
    pub align: Align,
    pub position: Position,
    pub vertical: bool,
    /// 背景条不透明度（0–255）。
    pub background_alpha: u8,
    /// 最后一条字幕驻留秒数（0 = 常驻）。
    pub linger: f64,
    /// 打印调试信息到 stderr。
    pub debug: bool,
}

/// 读线程配置。
#[derive(Debug, Clone, Copy)]
pub struct ReaderOptions {
    pub format: InputFormat,
    pub policy: TextPolicy,
}
