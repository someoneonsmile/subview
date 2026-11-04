//! 把字幕状态排成屏幕上的物理行（后端无关）。

use unicode_width::UnicodeWidthStr;

use crate::state::CaptionState;
use crate::wrap::wrap_text;

/// 水平对齐方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
}

/// 一行待渲染的字幕。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledLine {
    /// 说话人标签（只出现在一条字幕的第一行）。
    pub speaker: Option<String>,
    /// 行文本（已含对齐空格与悬挂缩进）。
    pub text: String,
    /// 是否为未定稿的中间结果。
    pub partial: bool,
    /// 淡出程度：0 = 正常，100 = 完全淡出。由 linger 逻辑驱动。
    pub fade: u8,
}

/// 排版参数。
pub struct LayoutOptions {
    /// 可用宽度（列）。
    pub width: usize,
    /// 最多保留的物理行数（通常等于终端高度）。
    pub max_lines: usize,
    pub align: Align,
    /// 应用于所有行的淡出程度。
    pub fade: u8,
}

/// 把当前字幕状态排成物理行，返回顺序为从屏幕最上方到最下方。
pub fn layout(state: &CaptionState, options: &LayoutOptions) -> Vec<StyledLine> {
    let width = options.width.max(1);
    let mut out: Vec<StyledLine> = Vec::new();

    let captions: Vec<(&crate::protocol::Caption, bool)> = state
        .history()
        .map(|caption| (caption, false))
        .chain(state.partial().map(|caption| (caption, true)))
        .collect();

    for (caption, partial) in captions {
        for (index, text) in wrap_text(&caption.text, width).into_iter().enumerate() {
            out.push(StyledLine {
                speaker: if index == 0 {
                    caption.speaker.clone()
                } else {
                    None
                },
                text,
                partial,
                fade: options.fade,
            });
        }
    }

    // 超出屏幕高度时保留最新的尾部。
    if out.len() > options.max_lines {
        out.drain(..out.len() - options.max_lines);
    }

    if options.align == Align::Center {
        for line in &mut out {
            let mut line_width = line.text.width();
            if let Some(speaker) = &line.speaker {
                line_width += speaker_prefix(speaker).width();
            }
            let padding = width.saturating_sub(line_width) / 2;
            if padding > 0 {
                line.text = format!("{}{}", " ".repeat(padding), line.text);
            }
        }
    }

    out
}

/// 说话人前缀文本，例如 `[A] `。
pub fn speaker_prefix(speaker: &str) -> String {
    format!("[{speaker}] ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Caption, Event};

    fn caption(text: &str) -> Caption {
        Caption {
            text: text.to_string(),
            speaker: None,
        }
    }

    fn options(width: usize, max_lines: usize, align: Align) -> LayoutOptions {
        LayoutOptions {
            width,
            max_lines,
            align,
            fade: 0,
        }
    }

    #[test]
    fn history_then_partial_order() {
        let mut state = CaptionState::new(3);
        state.apply(Event::Final(caption("一")));
        state.apply(Event::Partial(caption("二")));
        let lines = layout(&state, &options(10, 10, Align::Left));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "一");
        assert!(!lines[0].partial);
        assert_eq!(lines[1].text, "二");
        assert!(lines[1].partial);
    }

    #[test]
    fn lines_are_trimmed_to_max_lines() {
        let mut state = CaptionState::new(5);
        for text in ["一", "二", "三"] {
            state.apply(Event::Final(caption(text)));
        }
        let lines = layout(&state, &options(10, 2, Align::Left));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "二");
        assert_eq!(lines[1].text, "三");
    }

    #[test]
    fn center_alignment_pads_evenly() {
        let mut state = CaptionState::new(1);
        state.apply(Event::Final(caption("你好")));
        let lines = layout(&state, &options(10, 1, Align::Center));
        assert_eq!(lines[0].text, "   你好");
    }

    #[test]
    fn speaker_prefix_is_on_first_line_only() {
        let mut state = CaptionState::new(1);
        state.apply(Event::Final(Caption {
            text: "abcdefgh".to_string(),
            speaker: Some("A".to_string()),
        }));
        let lines = layout(&state, &options(6, 10, Align::Left));
        assert_eq!(lines[0].speaker.as_deref(), Some("A"));
        assert_eq!(lines[1].speaker, None);
    }

    #[test]
    fn fade_is_propagated() {
        let mut state = CaptionState::new(1);
        state.apply(Event::Final(caption("一")));
        let mut opts = options(10, 10, Align::Left);
        opts.fade = 60;
        let lines = layout(&state, &opts);
        assert_eq!(lines[0].fade, 60);
    }
}
