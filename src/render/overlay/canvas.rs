//! 像素层：画布混合、圆角背景、描边 / 阴影遮罩与字节序打包。

use cosmic_text::Color;

/// 从字形像素颜色中取 alpha，并应用 linger 淡出系数。
pub(super) fn scaled_alpha(color: Color, fade_factor: f64) -> u8 {
    let (_, _, _, alpha) = color.as_rgba_tuple();
    (f64::from(alpha) * fade_factor) as u8
}

/// 在画布上铺一层半透明圆角背景条。
pub(super) fn draw_background(frame: &mut [u32], width: i32, height: i32, radius: i32, alpha: u8) {
    if alpha == 0 || width <= 0 || height <= 0 {
        return;
    }
    let radius = radius.min(width / 2).min(height / 2).max(0);
    let inner_x = width - radius - 1;
    let inner_y = height - radius - 1;
    for y in 0..height {
        for x in 0..width {
            let cx = x.clamp(radius, inner_x);
            let cy = y.clamp(radius, inner_y);
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            let index = (y * width + x) as usize;
            blend_pixel(frame, index, 0, 0, 0, alpha);
        }
    }
}

/// 用可分离的最大值滤波把覆盖率向外扩张 `radius` 像素，生成清晰描边的遮罩。
pub(super) fn outline_mask(mask: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    if radius == 0 || width == 0 || height == 0 {
        return mask.to_vec();
    }
    let mut horizontal = vec![0u8; mask.len()];
    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let start = x.saturating_sub(radius);
            let end = (x + radius).min(width - 1);
            let mut peak = 0u8;
            for nx in start..=end {
                peak = peak.max(mask[row + nx]);
            }
            horizontal[row + x] = peak;
        }
    }
    let mut outline = vec![0u8; mask.len()];
    for x in 0..width {
        for y in 0..height {
            let start = y.saturating_sub(radius);
            let end = (y + radius).min(height - 1);
            let mut peak = 0u8;
            for ny in start..=end {
                peak = peak.max(horizontal[ny * width + x]);
            }
            outline[y * width + x] = peak;
        }
    }
    outline
}

/// 对遮罩做多遍盒式模糊（水平 + 垂直），生成柔和阴影边缘。
pub(super) fn blur_mask(
    mask: &mut [u8],
    width: usize,
    height: usize,
    radius: usize,
    passes: usize,
) {
    if radius == 0 || passes == 0 || width == 0 || height == 0 {
        return;
    }
    let mut scratch = vec![0u8; mask.len()];
    for _ in 0..passes {
        for y in 0..height {
            let row = y * width;
            for x in 0..width {
                let start = x.saturating_sub(radius);
                let end = (x + radius).min(width - 1);
                let mut sum = 0u32;
                for nx in start..=end {
                    sum += u32::from(mask[row + nx]);
                }
                scratch[row + x] = (sum / (end - start + 1) as u32) as u8;
            }
        }
        for x in 0..width {
            for y in 0..height {
                let start = y.saturating_sub(radius);
                let end = (y + radius).min(height - 1);
                let mut sum = 0u32;
                for ny in start..=end {
                    sum += u32::from(scratch[ny * width + x]);
                }
                mask[y * width + x] = (sum / (end - start + 1) as u32) as u8;
            }
        }
    }
}

/// 把一个遮罩（可带偏移）按 alpha 混合成深色到画布上，用于描边与阴影。
pub(super) fn blend_mask(
    frame: &mut [u32],
    mask: &[u8],
    width: i32,
    height: i32,
    offset_x: i32,
    offset_y: i32,
    alpha_scale: u8,
) {
    for y in 0..height {
        for x in 0..width {
            let value = mask[(y * width + x) as usize];
            if value == 0 {
                continue;
            }
            let target_x = x + offset_x;
            let target_y = y + offset_y;
            if target_x < 0 || target_x >= width || target_y < 0 || target_y >= height {
                continue;
            }
            let index = (target_y * width + target_x) as usize;
            let alpha = (u32::from(value) * u32::from(alpha_scale) / 255) as u8;
            blend_pixel(frame, index, 0, 0, 0, alpha);
        }
    }
}

/// 把 premultiplied ARGB 像素打包成 X11 `PutImage` 需要的字节序。
pub(super) fn pixels_to_bytes(pixels: &[u32], lsb_first: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(pixels.len() * 4);
    for &pixel in pixels {
        let alpha = ((pixel >> 24) & 0xff) as u8;
        let red = ((pixel >> 16) & 0xff) as u8;
        let green = ((pixel >> 8) & 0xff) as u8;
        let blue = (pixel & 0xff) as u8;
        if lsb_first {
            bytes.extend_from_slice(&[blue, green, red, alpha]);
        } else {
            bytes.extend_from_slice(&[alpha, red, green, blue]);
        }
    }
    bytes
}

/// 把一个像素按 premultiplied alpha 混合到透明的 ARGB 画布上。
pub(super) fn blend_pixel(
    frame: &mut [u32],
    index: usize,
    red: u8,
    green: u8,
    blue: u8,
    alpha: u8,
) {
    if alpha == 0 {
        return;
    }
    let source_alpha = u32::from(alpha);
    let inverse = 255 - source_alpha;
    let destination = frame[index];
    let destination_alpha = (destination >> 24) & 0xff;
    let out_alpha = source_alpha + destination_alpha * inverse / 255;
    let source_red = u32::from(red) * source_alpha / 255;
    let source_green = u32::from(green) * source_alpha / 255;
    let source_blue = u32::from(blue) * source_alpha / 255;
    let out_red = source_red + ((destination >> 16) & 0xff) * inverse / 255;
    let out_green = source_green + ((destination >> 8) & 0xff) * inverse / 255;
    let out_blue = source_blue + (destination & 0xff) * inverse / 255;
    frame[index] = (out_alpha << 24) | (out_red << 16) | (out_green << 8) | out_blue;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_mask_expands_coverage_by_radius() {
        let (width, height) = (7usize, 7usize);
        let mut mask = vec![0u8; width * height];
        mask[3 * width + 3] = 200;

        let outline = outline_mask(&mask, width, height, 1);
        // 中心保持原值，四周一圈被扩张为同一覆盖率。
        assert_eq!(outline[3 * width + 3], 200);
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let x = (3 + dx) as usize;
                let y = (3 + dy) as usize;
                assert_eq!(outline[y * width + x], 200);
            }
        }
        // 半径之外不受影响。
        assert_eq!(outline[0], 0);
        assert_eq!(outline[5 * width + 3], 0);

        // radius = 0 时是恒等变换。
        assert_eq!(outline_mask(&mask, width, height, 0), mask);
    }

    #[test]
    fn background_alpha_controls_opacity() {
        let (width, height) = (40usize, 40usize);
        let mut frame = vec![0u32; width * height];
        draw_background(&mut frame, width as i32, height as i32, 0, 30);
        // premultiplied ARGB：黑色、不透明度 30。
        assert_eq!(frame[(height / 2) * width + width / 2] >> 24, 30);

        // 0 = 完全透明，不写入任何像素（只有文字/阴影会留下非零像素）。
        let mut transparent = vec![0u32; width * height];
        draw_background(&mut transparent, width as i32, height as i32, 0, 0);
        assert!(transparent.iter().all(|&pixel| pixel == 0));
    }
}
