//! 渲染后端：把字幕状态画到屏幕上。

pub mod ansi;
pub mod layout;
#[cfg(feature = "overlay")]
pub mod overlay;
pub mod raw;
pub mod tui;
#[cfg(feature = "overlay")]
pub mod vertical;

use std::io;

use crate::state::CaptionState;

/// 所有屏幕后端共用的绘制接口。
pub trait Backend {
    /// 把当前字幕状态画到屏幕上。
    ///
    /// `fade` 为 linger 淡出进度（0–100），后端可据此调整样式。
    fn draw(&mut self, state: &CaptionState, cols: u16, rows: u16, fade: u8) -> io::Result<()>;
}
