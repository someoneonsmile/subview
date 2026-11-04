//! 桌面悬浮窗后端（默认，feature = "overlay"）。
//!
//! 用 X11 原生 32 位 ARGB 窗口实现：
//!
//! - **透明背景**：窗口 `background_pixel = 0`，直接 `PutImage` 32 位
//!   premultiplied alpha 像素，由合成器（picom / mutter / kwin 等）完成混合
//! - **点击穿透**：SHAPE 扩展把窗口的输入区域设为空
//! - **置顶 / 无边框 / 不抢焦点**：`override_redirect` 窗口，窗口管理器不接管
//!
//! 文字用 cosmic-text 排版与光栅化：先收集字形覆盖率画一层深色描边，
//! 再按颜色混合文字本身，保证字幕在亮背景上依然可读。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cosmic_text::{Attrs, Buffer, Color, FontSystem, Metrics, Shaping, SwashCache};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
use x11rb::protocol::xproto::{
    ClipOrdering, ColormapAlloc, ConnectionExt as XprotoConnectionExt, CreateGCAux,
    CreateWindowAux, EventMask, Gcontext, ImageFormat, ImageOrder, PropMode, Screen, VisualClass,
    Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

use crate::cli::{Cli, PositionArg};
use crate::protocol::{Caption, InputFormat};
use crate::reader;
use crate::render::layout::Align;
use crate::signal;
use crate::state::CaptionState;

/// 字号（像素）。
const FONT_SIZE: f32 = 24.0;
/// 行高（像素）。
const LINE_HEIGHT: f32 = 32.0;
/// 水平内边距（像素）。
const PADDING_X: f32 = 16.0;
/// 垂直内边距（像素）。
const PADDING_Y: f32 = 10.0;
/// 窗口距屏幕底部的边距（像素）。
const SCREEN_MARGIN: i32 = 60;
/// 字幕条背景不透明度（0–255，越低越透）。
const BACKGROUND_ALPHA: u8 = 30;
/// 字幕条圆角半径（像素）。
const CORNER_RADIUS: i32 = 10;
/// 默认窗口宽度按多少个全角字符估算。
const DEFAULT_MAX_CHARS: f32 = 15.0;
/// 已定稿字幕颜色。
const FINAL_COLOR: Color = Color::rgb(255, 255, 255);
/// 阴影水平偏移（像素）。
const SHADOW_OFFSET_X: i32 = 0;
/// 阴影垂直偏移（像素）。
const SHADOW_OFFSET_Y: i32 = 2;
/// 阴影模糊半径（像素）。
const SHADOW_BLUR_RADIUS: usize = 5;
/// 阴影模糊迭代次数（越大越柔和）。
const SHADOW_BLUR_PASSES: usize = 3;
/// 阴影不透明度（0–255）。
const SHADOW_ALPHA: u8 = 150;
/// linger 触发后的淡出时长（秒）。
const FADE_SECS: f64 = 0.8;
/// 主循环轮询间隔。
const TICK: Duration = Duration::from_millis(16);

/// 悬浮窗启动结果。
pub enum Outcome {
    /// 正常退出，携带进程退出码。
    Exited(i32),
    /// 图形环境不可用，调用方可以回退到终端界面。
    Unavailable(String),
}

/// 启动悬浮窗并接管主循环。
pub fn run(cli: &Cli, format: InputFormat, strip_control: bool) -> Outcome {
    let mut context = match WindowContext::new(cli) {
        Ok(context) => context,
        Err(error) => return Outcome::Unavailable(error),
    };
    match context.event_loop(cli, format, strip_control) {
        Ok(code) => Outcome::Exited(code),
        Err(error) => {
            eprintln!("subview: 悬浮窗渲染失败：{error}");
            Outcome::Exited(1)
        }
    }
}

/// 每帧的渲染选项。
struct RenderOptions {
    align: Align,
    fade: u8,
}

struct WindowContext {
    connection: RustConnection,
    window: Window,
    gc: Gcontext,
    width: u16,
    height: u16,
    lsb_first: bool,
    font_system: FontSystem,
    swash_cache: SwashCache,
}

impl WindowContext {
    fn new(cli: &Cli) -> Result<Self, String> {
        let (connection, screen_number) =
            x11rb::connect(None).map_err(|error| format!("无法连接 X11 图形环境：{error}"))?;
        let screen = &connection.setup().roots[screen_number];
        let root = screen.root;
        let lsb_first = connection.setup().image_byte_order == ImageOrder::LSB_FIRST;

        let visual_id = find_argb_visual(screen)
            .ok_or_else(|| "X11 服务器不提供 32 位 ARGB visual".to_string())?;
        warn_if_no_compositor(&connection);

        let screen_width = i32::from(screen.width_in_pixels);
        let screen_height = i32::from(screen.height_in_pixels);
        let max_width = (screen_width - 80).max(320);
        let default_width = (FONT_SIZE * 1.05 * DEFAULT_MAX_CHARS + 2.0 * PADDING_X).round() as i32;
        let width = match cli.width {
            // 悬浮窗模式下 --width 表示窗口宽度（像素）；不传时按约 15 个全角字估算。
            Some(requested) => i32::from(requested).clamp(200, max_width),
            None => default_width.min(max_width),
        };
        let width = width as u16;
        let lines = i32::from(cli.lines);
        let height = (lines * LINE_HEIGHT as i32 + 2 * PADDING_Y as i32) as u16;
        let x = ((screen_width - i32::from(width)) / 2) as i16;
        let y = match cli.position {
            PositionArg::Bottom => (screen_height - i32::from(height) - SCREEN_MARGIN) as i16,
            PositionArg::Top => SCREEN_MARGIN as i16,
        };

        let colormap = connection
            .generate_id()
            .map_err(|error| error.to_string())?;
        connection
            .create_colormap(ColormapAlloc::NONE, colormap, root, visual_id)
            .map_err(|error| format!("无法创建颜色表：{error}"))?;

        let window = connection
            .generate_id()
            .map_err(|error| error.to_string())?;
        let attributes = CreateWindowAux::new()
            .background_pixel(0u32)
            .border_pixel(0u32)
            .colormap(colormap)
            .override_redirect(1u32)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY);
        connection
            .create_window(
                32,
                window,
                root,
                x,
                y,
                width,
                height,
                0,
                WindowClass::INPUT_OUTPUT,
                visual_id,
                &attributes,
            )
            .map_err(|error| format!("无法创建透明窗口：{error}"))?;

        set_window_title(&connection, window);

        // 点击穿透：把窗口输入区域设为空。
        connection
            .shape_rectangles(
                SO::SET,
                SK::INPUT,
                ClipOrdering::UNSORTED,
                window,
                0,
                0,
                &[],
            )
            .map_err(|error| format!("无法设置窗口输入区域：{error}"))?;

        let gc = connection
            .generate_id()
            .map_err(|error| error.to_string())?;
        connection
            .create_gc(
                gc,
                window,
                &CreateGCAux::new().foreground(0u32).background(0u32),
            )
            .map_err(|error| format!("无法创建图形上下文：{error}"))?;

        connection
            .map_window(window)
            .map_err(|error| format!("无法映射窗口：{error}"))?;
        connection
            .flush()
            .map_err(|error| format!("无法刷新 X11 连接：{error}"))?;

        if cli.debug {
            eprintln!(
                "subview: overlay 窗口 id=0x{window:x} geometry={width}x{height}+{x}+{y} visual=0x{visual_id:x} lsb_first={lsb_first}"
            );
        }

        Ok(Self {
            connection,
            window,
            gc,
            width,
            height,
            lsb_first,
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
        })
    }

    fn event_loop(
        &mut self,
        cli: &Cli,
        format: InputFormat,
        strip_control: bool,
    ) -> Result<i32, String> {
        let state = Arc::new(Mutex::new(CaptionState::new(cli.lines as usize)));
        let dirty = Arc::new(AtomicBool::new(false));
        let eof = Arc::new(AtomicBool::new(false));
        let dirty_reader = dirty.clone();
        let eof_reader = eof.clone();
        reader::spawn_reader(state.clone(), format, strip_control, move |event| {
            dirty_reader.store(true, Ordering::Release);
            if event == reader::ReaderEvent::Finished {
                eof_reader.store(true, Ordering::Release);
            }
        });

        let linger_enabled = cli.linger > 0.0;
        let align: Align = cli.align.into();
        let mut last_event = Instant::now();
        let mut fade = 0u8;
        let mut linger_done = !linger_enabled;
        let mut needs_render = true;

        loop {
            if signal::stop_requested() {
                break;
            }

            // 处理挂起的 X11 事件。
            let mut exposed = false;
            loop {
                match self.connection.poll_for_event() {
                    Ok(Some(event)) => match event {
                        Event::Expose(expose) => {
                            if expose.count == 0 {
                                exposed = true;
                            }
                        }
                        Event::DestroyNotify(_) => {
                            return Ok(signal::exit_code());
                        }
                        _ => {}
                    },
                    Ok(None) => break,
                    Err(error) => return Err(error.to_string()),
                }
            }

            let changed = dirty.swap(false, Ordering::AcqRel);
            if changed {
                last_event = Instant::now();
                fade = 0;
                linger_done = !linger_enabled;
            }

            if !linger_done {
                let elapsed = last_event.elapsed().as_secs_f64();
                if elapsed > cli.linger {
                    let progress = ((elapsed - cli.linger) / FADE_SECS).clamp(0.0, 1.0);
                    let new_fade = (progress * 100.0).round() as u8;
                    if new_fade != fade {
                        fade = new_fade;
                        needs_render = true;
                    }
                    if progress >= 1.0 {
                        if let Ok(mut state) = state.lock() {
                            state.clear_display();
                        }
                        linger_done = true;
                        needs_render = true;
                    }
                }
            }

            if needs_render || changed || exposed {
                needs_render = false;
                let state = state.lock().map_err(|_| "字幕状态锁已中毒".to_string())?;
                let options = RenderOptions { align, fade };
                self.render(&state, &options)?;
            }

            if eof.load(Ordering::Acquire) && !dirty.load(Ordering::Acquire) {
                let state = state.lock().map_err(|_| "字幕状态锁已中毒".to_string())?;
                let options = RenderOptions { align, fade };
                self.render(&state, &options)?;
                let _ = self.connection.destroy_window(self.window);
                let _ = self.connection.flush();
                return Ok(signal::exit_code());
            }

            thread::sleep(TICK);
        }

        let _ = self.connection.destroy_window(self.window);
        let _ = self.connection.flush();
        Ok(signal::exit_code())
    }

    fn render(&mut self, state: &CaptionState, options: &RenderOptions) -> Result<(), String> {
        let width = self.width;
        let height = self.height;
        let mut frame = vec![0u32; width as usize * height as usize];
        self.draw_captions(&mut frame, state, options);
        let bytes = pixels_to_bytes(&frame, self.lsb_first);
        self.connection
            .put_image(
                ImageFormat::Z_PIXMAP,
                self.window,
                self.gc,
                width,
                height,
                0,
                0,
                0,
                32,
                &bytes,
            )
            .map_err(|error| format!("PutImage 失败：{error}"))?;
        self.connection
            .flush()
            .map_err(|error| format!("X11 刷新失败：{error}"))
    }

    fn draw_captions(&mut self, frame: &mut [u32], state: &CaptionState, options: &RenderOptions) {
        let text = compose_stream(state);
        if text.is_empty() {
            return;
        }

        let width = i32::from(self.width);
        let height = i32::from(self.height);
        let fade_factor = f64::from(100 - options.fade) / 100.0;

        // 半透明圆角背景条（类似 yt-live-sub）；linger 淡出时一起变淡。
        let background_alpha =
            (u32::from(BACKGROUND_ALPHA) * u32::from(100 - options.fade) / 100) as u8;
        draw_background(frame, width, height, CORNER_RADIUS, background_alpha);

        let content_width = (width as f32 - 2.0 * PADDING_X).max(1.0);
        let content_height = (height as f32 - 2.0 * PADDING_Y).max(LINE_HEIGHT);
        let capacity = ((content_height / LINE_HEIGHT) as usize).max(1);
        let alignment = match options.align {
            Align::Left => None,
            Align::Center => Some(cosmic_text::Align::Center),
        };

        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        // 高度不设限：先排完全部行，再整体上移，只显示最后 capacity 行。
        buffer.set_size(Some(content_width), None);
        buffer.set_text(&text, &Attrs::new(), Shaping::Advanced, alignment);
        buffer.shape_until_scroll(&mut self.font_system, false);

        // 内容按连续文本流折行；超出窗口时从顶部上移，让最新行留在窗口内
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
            &mut self.font_system,
            &mut self.swash_cache,
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

        // 第二遍：按文字颜色混合字形本体。
        buffer.draw(
            &mut self.font_system,
            &mut self.swash_cache,
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
}

/// 从字形像素颜色中取 alpha，并应用 linger 淡出系数。
fn scaled_alpha(color: Color, fade_factor: f64) -> u8 {
    let (_, _, _, alpha) = color.as_rgba_tuple();
    (f64::from(alpha) * fade_factor) as u8
}

/// 在画布上铺一层半透明圆角背景条。
fn draw_background(frame: &mut [u32], width: i32, height: i32, radius: i32, alpha: u8) {
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

/// 对遮罩做多遍盒式模糊（水平 + 垂直），生成柔和阴影边缘。
fn blur_mask(mask: &mut [u8], width: usize, height: usize, radius: usize, passes: usize) {
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
fn blend_mask(
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

/// 把最近的字幕拼成一条连续文本流（类似 yt-live-sub 的显示模型）。
///
/// 只保留尾部一段，既够折行显示最新几行，也避免长文本的排版开销。
fn compose_stream(state: &CaptionState) -> String {
    const MAX_CHARS: usize = 400;

    let mut text = String::new();
    for caption in state.history() {
        push_caption(&mut text, caption);
    }
    if let Some(caption) = state.partial() {
        push_caption(&mut text, caption);
    }
    if text.chars().count() > MAX_CHARS {
        let skip = text.chars().count() - MAX_CHARS;
        text = text.chars().skip(skip).collect();
    }
    text
}

fn push_caption(text: &mut String, caption: &Caption) {
    if !text.is_empty() {
        text.push(' ');
    }
    if let Some(speaker) = caption.speaker.as_deref().filter(|value| !value.is_empty()) {
        text.push_str(&format!("[{speaker}] "));
    }
    text.push_str(&caption.text);
}

/// 把 premultiplied ARGB 像素打包成 X11 `PutImage` 需要的字节序。
fn pixels_to_bytes(pixels: &[u32], lsb_first: bool) -> Vec<u8> {
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
fn blend_pixel(frame: &mut [u32], index: usize, red: u8, green: u8, blue: u8, alpha: u8) {
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

/// 在 AllowDepths 里找 32 位 TrueColor visual。
fn find_argb_visual(screen: &Screen) -> Option<u32> {
    screen
        .allowed_depths
        .iter()
        .find(|depth| depth.depth == 32)?
        .visuals
        .iter()
        .find(|visual| visual.class == VisualClass::TRUE_COLOR)
        .map(|visual| visual.visual_id)
}

/// 设置窗口标题（不影响透明与穿透，仅便于调试/工具识别）。
fn set_window_title(connection: &RustConnection, window: Window) {
    let _ = (|| -> Result<(), Box<dyn std::error::Error>> {
        let name_atom = connection
            .intern_atom(false, b"_NET_WM_NAME")?
            .reply()?
            .atom;
        let utf8_atom = connection.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
        connection.change_property8(PropMode::REPLACE, window, name_atom, utf8_atom, b"subview")?;
        Ok(())
    })();
}

/// 没有合成器时 ARGB 窗口不会真的透明，提前提示。
fn warn_if_no_compositor(connection: &RustConnection) {
    let result = (|| -> Result<bool, Box<dyn std::error::Error>> {
        let atom = connection
            .intern_atom(false, b"_NET_WM_CM_S0")?
            .reply()?
            .atom;
        let owner = connection.get_selection_owner(atom)?.reply()?.owner;
        Ok(owner == x11rb::NONE)
    })();
    if matches!(result, Ok(true)) {
        eprintln!("subview: warning: 未检测到合成器（compositor），透明背景可能显示为黑色");
        eprintln!("subview: 提示：启动 picom / mutter / kwin 等合成器以启用透明");
    }
}
