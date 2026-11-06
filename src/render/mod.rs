//! 渲染后端：把字幕状态画到屏幕上。

pub mod ansi;
pub mod layout;
#[cfg(feature = "overlay")]
pub mod overlay;
pub mod raw;
pub mod tui;

use std::io;

use crate::state::CaptionState;

/// linger 淡出进度：0 = 正常，100 = 完全淡出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Fade(u8);

impl Fade {
    pub const NONE: Self = Self(0);
    /// 完全淡出（由 `Linger` 在进度到达 100% 时产生；单测断言用）。
    #[cfg(test)]
    pub const FULL: Self = Self(100);

    pub fn new(percent: u8) -> Self {
        Self(percent.min(100))
    }

    pub fn percent(self) -> u8 {
        self.0
    }
}

/// 后端事件轮询结果。
///
/// `Redraw` / `Closed` 只由悬浮窗后端构造；纯终端构建下这两个变体仅出现在 `drive`
/// 的匹配分支里，不会产生值，因此允许 dead_code。
#[cfg_attr(not(feature = "overlay"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Poll {
    /// 没有需要处理的事件。
    #[default]
    Idle,
    /// 后端要求重绘（例如 X11 `Expose`）。
    Redraw,
    /// 后端的显示介质被关闭（例如 X11 窗口被销毁），主循环应退出。
    Closed,
}

/// 所有屏幕后端的统一绘制接口。
///
/// 统一发生在**内容层**：各后端自己决定物理行（终端）或像素（悬浮窗）的渲染方式，
/// 输出介质尺寸、颜色策略等由实现自己持有，`drive` 因此不依赖具体终端 API。
pub trait Renderer {
    /// 拉取后端特有事件。终端后端恒返回 [`Poll::Idle`]。
    fn poll(&mut self) -> io::Result<Poll> {
        Ok(Poll::Idle)
    }

    /// 把当前字幕状态画到屏幕上。
    fn draw(&mut self, state: &CaptionState, fade: Fade) -> io::Result<()>;
}

/// 终端尺寸（列、行）。
///
/// pty 未设置窗口大小时会返回 0×0，这种尺寸无法渲染，退回 80×24。
pub fn terminal_size() -> (u16, u16) {
    match crossterm::terminal::size() {
        Ok((cols, rows)) if cols > 0 && rows > 0 => (cols, rows),
        _ => (80, 24),
    }
}
