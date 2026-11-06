//! 竖排：切列算法与逐字框绘制（字从上到下、列从右到左）。

use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache, Weight};
use unicode_segmentation::UnicodeSegmentation;

use super::canvas::{
    blend_mask, blend_pixel, blur_mask, draw_background, outline_mask, scaled_alpha,
};
use super::compose::{Composed, ComposedLine};
use super::*;

/// 带「是否未定稿」标记的竖排列。
pub type TaggedColumn = Vec<(String, bool)>;

/// 把结构化行切成竖排列。
///
/// - `capacity`：每列最多容纳的 grapheme 数（由窗口高度决定）
/// - `max_columns`：最多保留的列数（由窗口宽度决定）
///
/// 每条字幕另起一列（行边界即换行），列从文本开头锚定、内容不会重排；
/// 只保留最新的 `max_columns` 列。返回顺序即显示顺序：索引 0 是最新
/// （窗口最左、可能未填满）一列，越靠后越旧（越靠右）。
///
/// 未定稿标记随行传递，不再依赖字节偏移；按 grapheme 边界切分，
/// 保证 CJK、emoji、组合字符不会被切开。
pub(super) fn split_columns_lines(
    lines: &[ComposedLine],
    capacity: usize,
    max_columns: usize,
) -> Vec<TaggedColumn> {
    if capacity == 0 || max_columns == 0 {
        return Vec::new();
    }

    let mut columns: Vec<TaggedColumn> = Vec::new();
    let mut current: TaggedColumn = Vec::new();
    for line in lines {
        for grapheme in line.text.graphemes(true) {
            current.push((grapheme.to_string(), line.partial));
            if current.len() == capacity {
                columns.push(std::mem::take(&mut current));
            }
        }
        // 每条字幕另起一列。
        if !current.is_empty() {
            columns.push(std::mem::take(&mut current));
        }
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

/// 竖排内容块的水平起点：列数不足一窗时整体贴右（竖排从右往左写，start 侧在右）。
pub(super) fn vertical_block_start_x(width: i32, columns: usize, cell: f32) -> f32 {
    (width as f32 - PADDING_X - columns as f32 * cell).max(PADDING_X)
}

/// 竖版字幕：字从上到下、列从右到左（中文传统竖排）。
///
/// cosmic-text 只有横排排版，这里把每个 grapheme 单独排进自己的字框并居中，
/// 再按“最新列在最左”的顺序逐列绘制；列不足一窗时整体贴右（竖排 start 侧在右）。
/// 窗口高度决定每列字数，宽度决定列数。
pub(super) fn draw(
    frame: &mut [u32],
    width: u16,
    height: u16,
    stream: &Composed,
    options: &RenderOptions,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
) {
    let width = i32::from(width);
    let height = i32::from(height);
    let fade_factor = f64::from(100 - options.fade) / 100.0;

    // 半透明圆角背景条；linger 淡出时一起变淡。
    let background_alpha =
        (u32::from(options.background_alpha) * u32::from(100 - options.fade) / 100) as u8;
    draw_background(frame, width, height, CORNER_RADIUS, background_alpha);

    let cell = LINE_HEIGHT;
    let content_width = (width as f32 - 2.0 * PADDING_X).max(1.0);
    let content_height = (height as f32 - 2.0 * PADDING_Y).max(1.0);
    let capacity = ((content_height / cell) as usize).max(1);
    let max_columns = ((content_width / cell) as usize).max(1);
    let columns = split_columns_lines(&stream.lines, capacity, max_columns);
    if columns.is_empty() {
        return;
    }

    // 单字一个字框：buffer 宽高都是字框边长，让它横向居中排一个字。
    let mut buffer = Buffer::new(font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
    buffer.set_size(Some(cell), Some(cell));

    let mut coverage = vec![0u8; frame.len()];
    let block_x = vertical_block_start_x(width, columns.len(), cell);
    for (column_index, column) in columns.iter().enumerate() {
        let cell_x = block_x + column_index as f32 * cell;
        for (row_index, (grapheme, partial)) in column.iter().enumerate() {
            let partial = *partial;
            let cell_y = PADDING_Y + row_index as f32 * cell;
            // 中文竖排惯例：逗号、顿号、句号挪到字框右上角。
            let (shift_x, shift_y) =
                punctuation_shift(grapheme).map_or((0.0, 0.0), |(x, y)| (x * cell, y * cell));
            // 未定稿的字用细字重 + 半透明白；alpha 差异来自 glyph 颜色，融入覆盖率遮罩。
            let attrs = if partial {
                Attrs::new().weight(Weight::LIGHT)
            } else {
                Attrs::new()
            };
            buffer.set_text(
                grapheme,
                &attrs,
                Shaping::Advanced,
                Some(cosmic_text::Align::Center),
            );
            buffer.shape_until_scroll(font_system, false);
            let glyph_color = if partial { PARTIAL_COLOR } else { FINAL_COLOR };
            buffer.draw(
                font_system,
                swash_cache,
                glyph_color,
                |x, y, _, _, color| {
                    let alpha = scaled_alpha(color, fade_factor);
                    if alpha == 0 {
                        return;
                    }
                    let left = (cell_x + shift_x) as i32;
                    let top = (cell_y + shift_y) as i32;
                    let px = left + x;
                    let py = top + y;
                    // 裁到字框内，避免相邻字互相覆盖。
                    if px < cell_x as i32
                        || px >= (cell_x + cell) as i32
                        || py < cell_y as i32
                        || py >= (cell_y + cell) as i32
                        || px < 0
                        || py < 0
                        || px >= width
                        || py >= height
                    {
                        return;
                    }
                    let index = py as usize * width as usize + px as usize;
                    if coverage[index] < alpha {
                        coverage[index] = alpha;
                    }
                },
            );
        }
    }

    // 阴影：先做盒式模糊再按偏移量投影，让文字在亮背景上也可读。
    let mut shadow = coverage.clone();
    blur_mask(
        &mut shadow,
        width as usize,
        height as usize,
        SHADOW_BLUR_RADIUS,
        SHADOW_BLUR_PASSES,
    );
    blend_mask(
        frame,
        &shadow,
        width,
        height,
        SHADOW_OFFSET_X,
        SHADOW_OFFSET_Y,
        SHADOW_ALPHA,
    );

    // 清晰描边：与横版一致，给竖排字也加一圈硬边。
    let outline = outline_mask(&coverage, width as usize, height as usize, OUTLINE_RADIUS);
    blend_mask(frame, &outline, width, height, 0, 0, OUTLINE_ALPHA);

    // 字形本体：覆盖率遮罩已含淡出系数与未定稿的 alpha（定稿 100%、未定稿 70%）。
    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) as usize;
            let alpha = coverage[index];
            if alpha == 0 {
                continue;
            }
            blend_pixel(frame, index, 255, 255, 255, alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<ComposedLine> {
        text.split('\n')
            .map(|segment| ComposedLine {
                text: segment.to_string(),
                partial: false,
            })
            .collect()
    }

    fn tagged_lines(parts: &[(&str, bool)]) -> Vec<ComposedLine> {
        parts
            .iter()
            .map(|(text, partial)| ComposedLine {
                text: text.to_string(),
                partial: *partial,
            })
            .collect()
    }

    fn columns(text: &str, capacity: usize, max_columns: usize) -> Vec<Vec<String>> {
        split_columns_lines(&lines(text), capacity, max_columns)
            .into_iter()
            .map(|column| column.into_iter().map(|(grapheme, _)| grapheme).collect())
            .collect()
    }

    #[test]
    fn vertical_columns_hug_right_when_not_full() {
        assert_eq!(vertical_block_start_x(96, 1, LINE_HEIGHT), 48.0);
        assert_eq!(vertical_block_start_x(96, 2, LINE_HEIGHT), 16.0);
    }

    #[test]
    fn newest_column_comes_first() {
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
        // 行边界在竖排里表示另起一列，每条字幕从新列顶部开始。
        assert_eq!(
            columns("你好\n世界", 3, 2),
            vec![vec!["世", "界"], vec!["你", "好"]]
        );
    }

    #[test]
    fn wrapped_column_then_newline_starts_another_column() {
        // 一行超过 capacity 时先按容量跨列，行边界后从新列继续。
        assert_eq!(
            columns("abcd\nef", 3, 10),
            vec![vec!["e", "f"], vec!["d"], vec!["a", "b", "c"]]
        );
    }

    #[test]
    fn tagged_columns_mark_partial_graphemes() {
        // “你好”是定稿，“世界”是未定稿：标记应落在后两个字上。
        let tagged = split_columns_lines(&tagged_lines(&[("你好", false), ("世界", true)]), 2, 2);
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
        let tagged = split_columns_lines(&lines("你好\n世界"), 3, 2);
        assert_eq!(
            tagged,
            vec![
                vec![("世".to_string(), false), ("界".to_string(), false)],
                vec![("你".to_string(), false), ("好".to_string(), false)],
            ]
        );
    }

    #[test]
    fn partial_marking_survives_line_boundaries() {
        let tagged = split_columns_lines(&tagged_lines(&[("一", false), ("二", true)]), 3, 2);
        assert_eq!(
            tagged,
            vec![
                vec![("二".to_string(), true)],
                vec![("一".to_string(), false)],
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
