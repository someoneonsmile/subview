//! 终端控制序列原语。

/// 进入字幕模式：切换备用屏幕、隐藏光标、关闭自动换行。
pub const ENTER_TUI: &str = "\x1b[?1049h\x1b[?25l\x1b[?7l";

/// 离开字幕模式：复位样式、恢复自动换行、显示光标、回到主屏幕。
pub const EXIT_TUI: &str = "\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l";

/// 复位所有样式。
pub const RESET: &str = "\x1b[0m";

/// 复位前色（保留背景）。
pub const RESET_COLOR: &str = "\x1b[39m";

/// 说话人标签颜色：青色。
pub const SPEAKER_COLOR: &str = "\x1b[36m";

/// 清空整行。
pub const CLEAR_LINE: &str = "\x1b[2K";

/// 清屏并把光标移到左上角。
pub const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";

/// 根据一行字幕的状态生成样式序列（SGR）。
pub fn style_sequence(line: &crate::render::layout::StyledLine) -> String {
    let mut sequence = String::from(RESET);
    if line.partial {
        // 未定稿：变暗 + 斜体，与已定稿内容区分。
        sequence.push_str("\x1b[2m\x1b[3m");
    } else if line.fade >= 50 {
        sequence.push_str("\x1b[2m");
    }
    if line.fade >= 80 {
        // 接近淡出完成时改用暗灰色。
        sequence.push_str("\x1b[38;5;240m");
    }
    sequence
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::layout::StyledLine;

    fn line(partial: bool, fade: u8) -> StyledLine {
        StyledLine {
            speaker: None,
            text: "你好".to_string(),
            partial,
            fade,
        }
    }

    #[test]
    fn normal_line_resets_only() {
        assert_eq!(style_sequence(&line(false, 0)), RESET);
    }

    #[test]
    fn partial_line_is_dim_and_italic() {
        let sequence = style_sequence(&line(true, 0));
        assert!(sequence.contains("\x1b[2m"));
        assert!(sequence.contains("\x1b[3m"));
    }

    #[test]
    fn faded_line_gets_dim() {
        assert!(style_sequence(&line(false, 60)).contains("\x1b[2m"));
    }

    #[test]
    fn nearly_gone_line_gets_gray() {
        assert!(style_sequence(&line(false, 90)).contains("38;5;240"));
    }
}
