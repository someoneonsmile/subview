//! 字幕显示状态：已定稿历史 + 当前未定稿内容。

use std::collections::VecDeque;

use crate::protocol::{Caption, Event};

/// 历史保留的条数硬上限。
///
/// 仅用于限制内存（避免长时间运行时无限增长）；实际显示哪几行由渲染层按
/// 可见行数裁剪，与 `--lines` 无关。
pub const HISTORY_CAP: usize = 256;

/// 屏幕上要显示的字幕状态。
///
/// 历史只保留最近 `max_history` 条，内存占用与运行时长无关。
pub struct CaptionState {
    history: VecDeque<Caption>,
    partial: Option<Caption>,
    max_history: usize,
}

impl CaptionState {
    pub fn new(max_history: usize) -> Self {
        Self {
            history: VecDeque::with_capacity(max_history.min(16)),
            partial: None,
            max_history: max_history.max(1),
        }
    }

    /// 应用一个协议事件。
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Final(caption) => {
                self.partial = None;
                self.history.push_back(caption);
                while self.history.len() > self.max_history {
                    self.history.pop_front();
                }
            }
            Event::Partial(caption) => self.partial = Some(caption),
            Event::Clear => self.clear_display(),
        }
    }

    /// 已定稿字幕，按时间顺序（旧 → 新）。
    pub fn history(&self) -> impl Iterator<Item = &Caption> {
        self.history.iter()
    }

    /// 当前未定稿字幕。
    pub fn partial(&self) -> Option<&Caption> {
        self.partial.as_ref()
    }

    /// 清空显示内容（linger 淡出结束后使用）。
    pub fn clear_display(&mut self) {
        self.history.clear();
        self.partial = None;
    }
}

/// 说话人前缀文本，例如 `[A] `。
pub fn speaker_prefix(speaker: &str) -> String {
    format!("[{speaker}] ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caption(text: &str) -> Caption {
        Caption {
            text: text.to_string(),
            speaker: None,
        }
    }

    #[test]
    fn partial_is_replaced_by_final() {
        let mut state = CaptionState::new(3);
        state.apply(Event::Partial(caption("你")));
        assert_eq!(state.partial(), Some(&caption("你")));
        state.apply(Event::Final(caption("你好")));
        assert_eq!(state.partial(), None);
        assert_eq!(state.history().collect::<Vec<_>>(), vec![&caption("你好")]);
    }

    #[test]
    fn history_is_bounded() {
        let mut state = CaptionState::new(2);
        for text in ["一", "二", "三"] {
            state.apply(Event::Final(caption(text)));
        }
        assert_eq!(
            state.history().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            vec!["二", "三"]
        );
    }

    #[test]
    fn clear_resets_everything() {
        let mut state = CaptionState::new(3);
        state.apply(Event::Final(caption("一")));
        state.apply(Event::Partial(caption("二")));
        state.apply(Event::Clear);
        assert!(state.history().next().is_none());
        assert!(state.partial().is_none());
    }

    #[test]
    fn zero_history_is_coerced_to_one() {
        let mut state = CaptionState::new(0);
        state.apply(Event::Final(caption("一")));
        state.apply(Event::Final(caption("二")));
        assert_eq!(state.history().count(), 1);
    }
}
