//! 竖排布局：字从上到下、列从右到左（中文传统竖排）。
//!
//! 这里只做与字体无关的切列：把连续文本流按每列容量切成若干竖排列，
//! 只保留最新的若干列。具体字形光栅化由悬浮窗后端完成。

use unicode_segmentation::UnicodeSegmentation;

/// 带「是否未定稿」标记的竖排列。
pub type TaggedColumn = Vec<(String, bool)>;

/// 把文本流切成竖排列。
///
/// - `capacity`：每列最多容纳的 grapheme 数（由窗口高度决定）
/// - `max_columns`：最多保留的列数（由窗口宽度决定）
/// - `partial_start`：整串文本中未定稿部分的字节起点；`None` 表示全部为定稿
///
/// 列从文本开头开始切分（首列从顶部向下填满），因此新内容只会填满当前的
/// 最后一列、再开新列，已有列的内容不会重排；只会把过旧的列挤出窗口。
/// 文本里的换行表示另起一列：换行后的内容从新列的顶部开始，不与上一列续接。
/// 返回顺序即显示顺序：索引 0 是最新（窗口最左、可能未填满）一列，
/// 越靠后越旧（越靠右）。
///
/// 按 grapheme 边界切分，保证 CJK、emoji、组合字符不会被切开。
pub fn split_columns_tagged(
    text: &str,
    capacity: usize,
    max_columns: usize,
    partial_start: Option<usize>,
) -> Vec<TaggedColumn> {
    if capacity == 0 || max_columns == 0 {
        return Vec::new();
    }

    // 换行在竖排里对应“另起一列”：每条字幕都从新列的顶部开始。
    let mut columns: Vec<TaggedColumn> = Vec::new();
    let mut current: TaggedColumn = Vec::new();
    for (offset, grapheme) in text.grapheme_indices(true) {
        if grapheme == "\n" || grapheme == "\r" {
            if !current.is_empty() {
                columns.push(std::mem::take(&mut current));
            }
            continue;
        }
        let partial = partial_start.is_some_and(|start| offset >= start);
        current.push((grapheme.to_string(), partial));
        if current.len() == capacity {
            columns.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        columns.push(current);
    }

    // 只保留最新的 max_columns 列；显示时最新的列在最左。
    let skip = columns.len().saturating_sub(max_columns);
    let mut columns: Vec<TaggedColumn> = columns.into_iter().skip(skip).collect();
    columns.reverse();
    columns
}

/// 竖排中标点的位移（单位：字框边长），`None` 表示保持居中。
///
/// 中文竖排惯例：逗号、顿号、句号放在字框右上角，而不是像横排那样落在
/// 左下角。这里只处理随正文出现的标点；冒号、分号、叹号、问号在竖排中
/// 保持居中，不位移。
pub fn punctuation_shift(grapheme: &str) -> Option<(f32, f32)> {
    match grapheme {
        "，" | "。" | "、" => Some((0.55, -0.55)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(text: &str, capacity: usize, max_columns: usize) -> Vec<Vec<String>> {
        split_columns_tagged(text, capacity, max_columns, None)
            .into_iter()
            .map(|column| column.into_iter().map(|(grapheme, _)| grapheme).collect())
            .collect()
    }

    #[test]
    fn newest_column_comes_first() {
        // 6 个字、每列 2 个：只留最新 2 列，最新列在索引 0（最左）。
        assert_eq!(
            columns("abcdef", 2, 2),
            vec![vec!["e", "f"], vec!["c", "d"]]
        );
    }

    #[test]
    fn cjk_is_split_by_grapheme() {
        assert_eq!(
            columns("你好世界", 2, 2),
            vec![vec!["世", "界"], vec!["你", "好"]]
        );
    }

    #[test]
    fn short_text_yields_single_column() {
        assert_eq!(columns("你好", 8, 2), vec![vec!["你", "好"]]);
    }

    #[test]
    fn empty_text_yields_no_columns() {
        assert!(columns("", 8, 2).is_empty());
    }

    #[test]
    fn zero_capacity_or_columns_yields_nothing() {
        assert!(columns("你好", 0, 2).is_empty());
        assert!(columns("你好", 2, 0).is_empty());
    }

    #[test]
    fn column_with_single_capacity_keeps_latest_chars() {
        assert_eq!(columns("你好世界", 1, 2), vec![vec!["界"], vec!["世"]]);
    }

    #[test]
    fn emoji_and_combined_graphemes_are_not_split() {
        let family = "👨‍👩‍👧‍👦";
        let text = format!("你{family}好");
        assert_eq!(
            columns(&text, 1, 3),
            vec![vec!["好"], vec![family], vec!["你"]]
        );
    }

    #[test]
    fn partial_newest_column_is_leftmost() {
        // 7 个字、每列 3 个：最新一列只有 1 个字，且在最左（从顶部开始填）。
        assert_eq!(
            columns("abcdefg", 3, 2),
            vec![vec!["g"], vec!["d", "e", "f"]]
        );
    }

    #[test]
    fn existing_columns_do_not_reflow_when_text_grows() {
        // 列从文本开头锚定：新增内容只进入最新的列，旧列内容保持不动。
        let before = columns("abcdef", 3, 2);
        let after = columns("abcdefg", 3, 2);
        assert_eq!(before, vec![vec!["d", "e", "f"], vec!["a", "b", "c"]]);
        assert_eq!(after[1], before[0]);
    }

    #[test]
    fn newline_starts_a_new_column() {
        // 换行在竖排里表示另起一列，每条字幕从新列顶部开始。
        assert_eq!(
            columns("你好\n世界", 3, 2),
            vec![vec!["世", "界"], vec!["你", "好"]]
        );
    }

    #[test]
    fn wrapped_column_then_newline_starts_another_column() {
        // 一段超过 capacity 时先按容量跨列，换行后从新列继续。
        assert_eq!(
            columns("abcd\nef", 3, 10),
            vec![vec!["e", "f"], vec!["d"], vec!["a", "b", "c"]]
        );
    }

    #[test]
    fn tagged_columns_mark_partial_graphemes() {
        // “你好”是定稿，“世界”是未定稿：标记应落在后两个字上。
        let tagged = split_columns_tagged("你好世界", 2, 2, Some("你好".len()));
        assert_eq!(
            tagged,
            vec![
                vec![("世".to_string(), true), ("界".to_string(), true)],
                vec![("你".to_string(), false), ("好".to_string(), false)],
            ]
        );
    }

    #[test]
    fn tagged_columns_without_partial_are_all_final() {
        let tagged = split_columns_tagged("你好\n世界", 3, 2, None);
        assert_eq!(
            tagged,
            vec![
                vec![("世".to_string(), false), ("界".to_string(), false)],
                vec![("你".to_string(), false), ("好".to_string(), false)],
            ]
        );
    }

    #[test]
    fn tagged_columns_keep_newline_offset_correct() {
        // 换行后的第一个字偏移应为 4（“一”占 3 字节 + 换行 1 字节），按字节比较。
        let text = "一\n二";
        let tagged = split_columns_tagged(text, 3, 2, Some("一\n".len()));
        assert_eq!(
            tagged,
            vec![
                vec![("二".to_string(), true)],
                vec![("一".to_string(), false)]
            ]
        );
    }

    #[test]
    fn punctuation_is_shifted_in_vertical_mode() {
        assert!(punctuation_shift("，").is_some());
        assert!(punctuation_shift("。").is_some());
        assert!(punctuation_shift("、").is_some());
        assert!(punctuation_shift("你").is_none());
        assert!(punctuation_shift("？").is_none());
    }
}
