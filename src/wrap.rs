//! 按显示宽度折行。
//!
//! 使用 grapheme cluster 边界与 Unicode 显示宽度，保证中日韩字符、emoji、
//! 组合字符不会被从中间切开。续行带 2 列悬挂缩进。

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// 续行悬挂缩进列数。
const HANGING_INDENT: usize = 2;

/// 启用悬挂缩进的最小显示宽度；太窄时缩进只会挤占正文。
const HANGING_INDENT_MIN_WIDTH: usize = 16;

/// 制表符展开为空格数。
const TAB_WIDTH: usize = 4;

/// 把一段文本折成若干显示行，每行不超过 `width` 列。
///
/// 续行在宽度允许时缩进 [`HANGING_INDENT`] 列。行尾空白会被去掉。
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let text = expand_tabs(text);
    let width = width.max(1);
    let indent = if width > HANGING_INDENT_MIN_WIDTH {
        HANGING_INDENT
    } else {
        0
    };

    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut line_width = 0usize;

    for grapheme in text.graphemes(true) {
        let grapheme_width = grapheme.width();
        if line_width > 0 && line_width + grapheme_width > width {
            lines.push(line.trim_end().to_string());
            line = String::new();
            line_width = 0;
            if indent > 0 {
                line.push_str(&" ".repeat(HANGING_INDENT));
                line_width = indent;
            }
        }
        line.push_str(grapheme);
        line_width += grapheme_width;
    }

    if !line.is_empty() || lines.is_empty() {
        lines.push(line.trim_end().to_string());
    }
    lines
}

/// 把制表符展开为空格，避免终端把 `\t` 显示到不可预测的位置。
fn expand_tabs(text: &str) -> String {
    if !text.contains('\t') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '\t' {
            for _ in 0..TAB_WIDTH {
                out.push(' ');
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_wraps_on_spaces() {
        assert_eq!(wrap_text("hello world", 6), vec!["hello", "world"]);
    }

    #[test]
    fn cjk_wraps_on_character_boundaries() {
        assert_eq!(wrap_text("你好世界", 4), vec!["你好", "世界"]);
    }

    #[test]
    fn cjk_never_splits_a_wide_character() {
        assert_eq!(wrap_text("你好世界", 3), vec!["你", "好", "世", "界"]);
    }

    #[test]
    fn long_line_gets_hanging_indent() {
        assert_eq!(
            wrap_text("abcdefghijklmnopqrstuvwx", 20),
            vec!["abcdefghijklmnopqrst".to_string(), "  uvwx".to_string()]
        );
    }

    #[test]
    fn short_line_is_untouched() {
        assert_eq!(wrap_text("你好", 10), vec!["你好"]);
    }

    #[test]
    fn emoji_is_kept_whole() {
        // ZWJ 家庭 emoji：显示宽度 2，不能被拆成多个码点行。
        let family = "👨‍👩‍👧‍👦";
        assert_eq!(wrap_text(family, 2), vec![family]);
    }

    #[test]
    fn tabs_are_expanded() {
        assert_eq!(wrap_text("a\tb", 10), vec!["a    b"]);
    }

    #[test]
    fn empty_text_yields_one_empty_line() {
        assert_eq!(wrap_text("", 10), vec![""]);
    }

    #[test]
    fn line_ending_with_space_is_trimmed() {
        assert_eq!(wrap_text("hello world", 6), vec!["hello", "world"]);
    }
}
