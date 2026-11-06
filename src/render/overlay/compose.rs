//! 字幕状态 → 渲染用的结构化行。

use crate::protocol::Caption;
use crate::state::CaptionState;

/// 截断后保留的最大字符数（含行间换行）。
const MAX_CHARS: usize = 400;

/// 一行待渲染的内容。
pub(super) struct ComposedLine {
    pub(super) text: String,
    /// 是否来自未定稿的 partial 字幕。
    pub(super) partial: bool,
}

/// 组合后的内容：按行切分，每行带定稿标记。
pub(super) struct Composed {
    pub(super) lines: Vec<ComposedLine>,
}

impl Composed {
    pub(super) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// 拼成整串文本（行间以 `\n` 分隔）与未定稿段的字节起点。
    ///
    /// 横版排版需要整串文本；起点由行边界推导，不依赖截断后的偏移修正。
    pub(super) fn as_text(&self) -> (String, Option<usize>) {
        let mut text = String::new();
        let mut partial_start = None;
        for (index, line) in self.lines.iter().enumerate() {
            if index > 0 {
                text.push('\n');
            }
            if line.partial && partial_start.is_none() {
                partial_start = Some(text.len());
            }
            text.push_str(&line.text);
        }
        (text, partial_start)
    }
}

/// 把最近的字幕组合成结构化行（每条字幕独立成行，只保留最新的一段）。
///
/// 截断按行进行：先丢弃最旧的行，避免把某行切成两半；单行本身超长时保留尾部。
pub(super) fn compose(state: &CaptionState) -> Composed {
    let mut lines: Vec<ComposedLine> = Vec::new();
    for caption in state.history() {
        push_caption(&mut lines, caption, false);
    }
    if let Some(caption) = state.partial() {
        push_caption(&mut lines, caption, true);
    }

    truncate(&mut lines);
    Composed { lines }
}

/// 追加一条字幕；字幕内的换行拆成独立行，speaker 前缀只加在首行。
fn push_caption(lines: &mut Vec<ComposedLine>, caption: &Caption, partial: bool) {
    let speaker = caption.speaker.as_deref().filter(|value| !value.is_empty());
    for (index, segment) in caption.text.split('\n').enumerate() {
        let mut text = String::new();
        if index == 0
            && let Some(speaker) = speaker
        {
            text.push_str(&crate::state::speaker_prefix(speaker));
        }
        text.push_str(segment);
        lines.push(ComposedLine { text, partial });
    }
}

/// 总字符数：各行字符之和 + 行间换行。
fn char_count(lines: &[ComposedLine]) -> usize {
    lines
        .iter()
        .map(|line| line.text.chars().count())
        .sum::<usize>()
        + lines.len().saturating_sub(1)
}

/// 按行截断到 [`MAX_CHARS`]：从最旧的行开始整行丢弃；仅剩一行仍超长时保留尾部。
fn truncate(lines: &mut Vec<ComposedLine>) {
    let mut total = char_count(lines);
    while total > MAX_CHARS && lines.len() > 1 {
        let removed = lines.remove(0);
        total -= removed.text.chars().count() + 1;
    }
    if total > MAX_CHARS
        && let Some(first) = lines.first_mut()
    {
        let over = total - MAX_CHARS;
        first.text = first.text.chars().skip(over).collect();
    }
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

    fn state_with(finals: &[&str], partial: Option<&str>) -> CaptionState {
        let mut state = CaptionState::new(crate::state::HISTORY_CAP);
        for text in finals {
            state.apply(crate::protocol::Event::Final(caption(text)));
        }
        if let Some(text) = partial {
            state.apply(crate::protocol::Event::Partial(caption(text)));
        }
        state
    }

    #[test]
    fn each_caption_becomes_one_line() {
        let state = state_with(&["一", "二"], Some("三"));
        let composed = compose(&state);
        let texts: Vec<_> = composed
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(texts, ["一", "二", "三"]);
        assert_eq!(
            composed
                .lines
                .iter()
                .map(|line| line.partial)
                .collect::<Vec<_>>(),
            [false, false, true]
        );
    }

    #[test]
    fn as_text_joins_lines_and_marks_partial_start() {
        let state = state_with(&["一", "二"], Some("三"));
        let (text, partial_start) = compose(&state).as_text();
        assert_eq!(text, "一\n二\n三");
        assert_eq!(partial_start, Some("一\n二\n".len()));
    }

    #[test]
    fn speaker_prefix_goes_on_first_line_only() {
        let mut state = CaptionState::new(crate::state::HISTORY_CAP);
        state.apply(crate::protocol::Event::Final(Caption {
            text: "上\n下".to_string(),
            speaker: Some("A".to_string()),
        }));
        let composed = compose(&state);
        assert_eq!(composed.lines[0].text, "[A] 上");
        assert_eq!(composed.lines[1].text, "下");
    }

    #[test]
    fn truncation_drops_oldest_lines_whole() {
        let long = "b".repeat(399);
        let state = state_with(&[&long], Some("三"));
        let composed = compose(&state);
        // 401 字符超过上限：整行丢弃旧行，只留未定稿行。
        assert_eq!(composed.lines.len(), 1);
        assert_eq!(composed.lines[0].text, "三");
        assert!(composed.lines[0].partial);
    }

    #[test]
    fn single_overlong_line_keeps_tail() {
        let long = "b".repeat(MAX_CHARS + 10);
        let state = state_with(&[&long], None);
        let composed = compose(&state);
        assert_eq!(composed.lines.len(), 1);
        assert_eq!(composed.lines[0].text.chars().count(), MAX_CHARS);
        assert!(composed.lines[0].text.ends_with('b'));
    }

    #[test]
    fn empty_state_composes_to_nothing() {
        let state = CaptionState::new(crate::state::HISTORY_CAP);
        assert!(compose(&state).is_empty());
    }
}
