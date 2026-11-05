//! 终端后端：接管终端，在底部固定区域重绘字幕。

use std::io::{self, Write};

use crate::render::Backend;
use crate::render::ansi;
use crate::render::layout::{Align, LayoutOptions, StyledLine, layout};
use crate::state::CaptionState;

/// 在终端里显示字幕的渲染器。
///
/// 生命周期：先 [`TuiRenderer::enter`]，之后反复 [`Backend::draw`]，
/// 最后 [`TuiRenderer::exit`]（Drop 时也会兜底恢复终端）。
pub struct TuiRenderer<W: Write> {
    out: W,
    align: Align,
    width_limit: Option<usize>,
    color: bool,
    /// 最多显示的字幕行数。
    max_lines: usize,
    prev_start: usize,
    prev_height: usize,
    prev_rows: u16,
    active: bool,
}

impl<W: Write> TuiRenderer<W> {
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
            prev_start: 0,
            prev_height: 0,
            prev_rows: 0,
            active: false,
        }
    }

    /// 进入字幕模式。
    pub fn enter(&mut self) -> io::Result<()> {
        write!(self.out, "{}", ansi::ENTER_TUI)?;
        self.out.flush()?;
        self.active = true;
        Ok(())
    }

    /// 恢复终端状态。幂等。
    pub fn exit(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        write!(self.out, "{}", ansi::EXIT_TUI)?;
        self.out.flush()
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
                    crate::render::layout::speaker_prefix(speaker),
                    ansi::RESET_COLOR
                )?;
            } else {
                write!(
                    self.out,
                    "{}",
                    crate::render::layout::speaker_prefix(speaker)
                )?;
            }
        }
        write!(self.out, "{}", line.text)?;
        if self.color {
            write!(self.out, "{}", ansi::RESET)?;
        }
        Ok(())
    }
}

impl<W: Write> Backend for TuiRenderer<W> {
    fn draw(&mut self, state: &CaptionState, cols: u16, rows: u16, fade: u8) -> io::Result<()> {
        let cols = cols.max(1) as usize;
        let rows = rows.max(1) as usize;
        let width = self.width_limit.unwrap_or(cols).min(cols).max(1);
        let max_lines = rows.min(self.max_lines).max(1);
        let lines = layout(
            state,
            &LayoutOptions {
                width,
                max_lines,
                align: self.align,
                fade,
            },
        );
        let height = lines.len();

        // 终端尺寸变化时上一帧的位置信息失效，直接清屏重来。
        if self.prev_rows != 0 && self.prev_rows != rows as u16 {
            write!(self.out, "{}", ansi::CLEAR_SCREEN)?;
            self.prev_start = 0;
            self.prev_height = 0;
        }

        // 区域底部对齐；空状态下没有区域。
        let start = if height == 0 {
            rows + 1
        } else {
            rows - height + 1
        };

        // 清掉上一帧与这一帧覆盖过的所有行。
        let clear_from = if self.prev_height == 0 {
            start
        } else {
            start.min(self.prev_start)
        };
        let clear_to = (start + height).max(self.prev_start + self.prev_height);
        for row in clear_from..clear_to {
            if row > rows {
                break;
            }
            write!(self.out, "\x1b[{};1H{}", row, ansi::CLEAR_LINE)?;
        }

        for (index, line) in lines.iter().enumerate() {
            let row = start + index;
            write!(self.out, "\x1b[{};1H", row)?;
            self.draw_line(line)?;
        }

        write!(self.out, "{}", ansi::RESET)?;
        self.out.flush()?;

        self.prev_start = start;
        self.prev_height = height;
        self.prev_rows = rows as u16;
        Ok(())
    }
}

impl<W: Write> Drop for TuiRenderer<W> {
    fn drop(&mut self) {
        let _ = self.exit();
    }
}
