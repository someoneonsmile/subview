//! 横版悬浮窗绘制：cosmic-text 整段排版 + 光栅合成。

use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache, Weight};

use crate::render::layout::Align;

use super::canvas::{
    blend_mask, blend_pixel, blur_mask, draw_background, outline_mask, scaled_alpha,
};
use super::compose::Composed;
use super::*;

/// 横版绘制入口。
pub(super) fn draw(
    frame: &mut [u32],
    width: u16,
    height: u16,
    stream: &Composed,
    options: &RenderOptions,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
) {
    let (text, partial_start) = stream.as_text();
    let text = text.as_str();

    let width = i32::from(width);
    let height = i32::from(height);
    let fade_factor = f64::from(100 - options.fade) / 100.0;

    // 半透明圆角背景条（类似 yt-live-sub）；linger 淡出时一起变淡。
    let background_alpha =
        (u32::from(options.background_alpha) * u32::from(100 - options.fade) / 100) as u8;
    draw_background(frame, width, height, CORNER_RADIUS, background_alpha);

    let content_width = (width as f32 - 2.0 * PADDING_X).max(1.0);
    let content_height = (height as f32 - 2.0 * PADDING_Y).max(LINE_HEIGHT);
    let capacity = ((content_height / LINE_HEIGHT) as usize).max(1);
    let alignment = match options.align {
        Align::Left => None,
        Align::Center => Some(cosmic_text::Align::Center),
    };

    let mut buffer = Buffer::new(font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
    // 高度不设限：先排完全部行，再整体上移，只显示最后 capacity 行。
    buffer.set_size(Some(content_width), None);
    // 已定稿白字、未定稿浅灰：分段设置颜色，排版仍按整段文本流折行。
    let final_attrs = Attrs::new().color(FINAL_COLOR);
    let partial_attrs = Attrs::new().color(PARTIAL_COLOR).weight(Weight::LIGHT);
    let mut spans: Vec<(&str, Attrs)> = Vec::new();
    match partial_start {
        Some(split) if split < text.len() => {
            if split > 0 {
                spans.push((&text[..split], final_attrs.clone()));
            }
            spans.push((&text[split..], partial_attrs));
        }
        _ => spans.push((text, final_attrs)),
    }
    buffer.set_rich_text(spans, &Attrs::new(), Shaping::Advanced, alignment);
    buffer.shape_until_scroll(font_system, false);

    // 内容按文本流折行（字幕间已换行）；超出窗口时从顶部上移，让最新行留在窗口内
    // （yt-live-sub 的“新内容从下方进入、旧行被顶上去”效果）。
    let rendered_lines = buffer.layout_runs().count() as f32;
    let total_height = rendered_lines * LINE_HEIGHT;
    let overflow = (total_height - content_height).max(0.0);
    let offset_x = PADDING_X as i32;
    let offset_y = (PADDING_Y - overflow) as i32;
    // 只绘制最后 capacity 行：整行裁掉顶部溢出的行，避免露出半截字形。
    let first_visible_y = (rendered_lines - capacity as f32).max(0.0) * LINE_HEIGHT;

    // 第一遍：收集字形覆盖率。
    let mut coverage = vec![0u8; frame.len()];
    buffer.draw(
        font_system,
        swash_cache,
        FINAL_COLOR,
        |x, y, _, _, color| {
            if (y as f32) < first_visible_y {
                return;
            }
            let alpha = scaled_alpha(color, fade_factor);
            let px = x + offset_x;
            let py = y + offset_y;
            if alpha == 0 || px < 0 || py < 0 || px >= width || py >= height {
                return;
            }
            let index = py as usize * width as usize + px as usize;
            if coverage[index] < alpha {
                coverage[index] = alpha;
            }
        },
    );

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

    // 清晰描边：白字在任何桌面背景上都有硬边，避免半透明背景发灰。
    let outline = outline_mask(&coverage, width as usize, height as usize, OUTLINE_RADIUS);
    blend_mask(frame, &outline, width, height, 0, 0, OUTLINE_ALPHA);

    // 第二遍：按文字颜色混合字形本体。
    buffer.draw(
        font_system,
        swash_cache,
        FINAL_COLOR,
        |x, y, _, _, color| {
            if (y as f32) < first_visible_y {
                return;
            }
            let alpha = scaled_alpha(color, fade_factor);
            let px = x + offset_x;
            let py = y + offset_y;
            if alpha == 0 || px < 0 || py < 0 || px >= width || py >= height {
                return;
            }
            let (red, green, blue, _) = color.as_rgba_tuple();
            let index = py as usize * width as usize + px as usize;
            blend_pixel(frame, index, red, green, blue, alpha);
        },
    );
}
