//! `--raw` 后端：不接管屏幕，原地重绘一段字幕区域。
//!
//! 与 [`crate::render::tui`] 的区别是不使用备用屏幕，可以和终端里已有的内容
//! 共存，适合嵌进 tmux 分屏或通过管道录制。

use std::io::{self, Write};

use crate::render::ansi;
use crate::render::layout::{Align, LayoutOptions, StyledLine, layout};
use crate::render::{Fade, Renderer};
use crate::state::CaptionState;

/// 原地重绘的过滤器渲染器。
pub struct RawRenderer<W: Write> {
    out: W,
    align: Align,
    width_limit: Option<usize>,
    color: bool,
    /// 最多显示的字幕行数。
    max_lines: usize,
    /// 当前光标所在行距区域顶部的距离（0 表示没有区域）。
    last_height: usize,
}

impl<W: Write> RawRenderer<W> {
    pub fn new(
        out: W,
        align: Align,
        width_limit: Option<usize>,
        color: bool,
        max_lines: usize,
    ) -> Self {
        Self {
            out,
            align,
            width_limit,
            color,
            max_lines: max_lines.max(1),
            last_height: 0,
        }
    }

    fn draw_line(&mut self, line: &StyledLine) -> io::Result<()> {
        if self.color {
            write!(self.out, "{}", ansi::style_sequence(line))?;
        }
        if let Some(speaker) = &line.speaker {
            if self.color {
                write!(
                    self.out,
                    "{}{}{}",
                    ansi::SPEAKER_COLOR,
                    crate::state::speaker_prefix(speaker),
                    ansi::RESET_COLOR
                )?;
            } else {
                write!(self.out, "{}", crate::state::speaker_prefix(speaker))?;
            }
        }
        write!(self.out, "{}", line.text)?;
        if self.color {
            write!(self.out, "{}", ansi::RESET)?;
        }
        Ok(())
    }

    /// 结束显示：保证 shell 提示符出现在新的一行。
    pub fn finish(&mut self) -> io::Result<()> {
        if self.last_height > 0 {
            write!(self.out, "\r\n")?;
            self.last_height = 0;
        }
        self.out.flush()
    }
}

impl<W: Write> Renderer for RawRenderer<W> {
    fn draw(&mut self, state: &CaptionState, fade: Fade) -> io::Result<()> {
        let (cols, rows) = crate::render::terminal_size();
        let cols = cols.max(1) as usize;
        let rows = rows.max(1) as usize;
        let width = self.width_limit.unwrap_or(cols).min(cols).max(1);
        let lines = layout(
            state,
            &LayoutOptions {
                width,
                max_lines: rows.min(self.max_lines).max(1),
                align: self.align,
                fade: fade.percent(),
            },
        );
        let height = lines.len();
        let clear_height = self.last_height.max(height);
        if clear_height == 0 {
            return Ok(());
        }

        // 回到区域第一行行首。
        write!(self.out, "\r")?;
        if self.last_height > 1 {
            write!(self.out, "\x1b[{}A", self.last_height - 1)?;
        }

        for index in 0..clear_height {
            write!(self.out, "{}", ansi::CLEAR_LINE)?;
            if let Some(line) = lines.get(index) {
                self.draw_line(line)?;
            }
            if index + 1 < clear_height {
                write!(self.out, "\r\n")?;
            }
        }

        write!(self.out, "{}", ansi::RESET)?;
        self.out.flush()?;
        self.last_height = clear_height;
        Ok(())
    }
}

impl<W: Write> Drop for RawRenderer<W> {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}
