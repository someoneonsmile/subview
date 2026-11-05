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

use cosmic_text::{Attrs, Buffer, Color, FontSystem, Metrics, Shaping, SwashCache, Weight};
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
use crate::render::vertical;
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
/// 字幕条圆角半径（像素）。
const CORNER_RADIUS: i32 = 10;
/// 已定稿字幕颜色。
const FINAL_COLOR: Color = Color::rgb(255, 255, 255);
/// 未定稿字幕颜色：半透明白（约 43%），配合细字重和已定稿的纯白字区分开。
const PARTIAL_COLOR: Color = Color::rgba(255, 255, 255, 110);
/// 阴影水平偏移（像素）。
const SHADOW_OFFSET_X: i32 = 0;
/// 阴影垂直偏移（像素）。
const SHADOW_OFFSET_Y: i32 = 1;
/// 阴影模糊半径（像素）。
const SHADOW_BLUR_RADIUS: usize = 3;
/// 阴影模糊迭代次数（越大越柔和）。
const SHADOW_BLUR_PASSES: usize = 2;
/// 阴影不透明度（0–255）。
const SHADOW_ALPHA: u8 = 90;
/// 描边向外扩张的半径（像素）：给字加一圈硬边，提高任意背景下的对比度。
const OUTLINE_RADIUS: usize = 1;
/// 描边不透明度（0–255）。
const OUTLINE_ALPHA: u8 = 120;
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
    /// 背景条不透明度（0–255，淡出时按进度递减）。
    background_alpha: u8,
}

/// 悬浮窗几何（像素）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Geometry {
    width: u16,
    height: u16,
    x: i16,
    y: i16,
}

/// 根据屏幕尺寸与命令行参数计算悬浮窗几何。
///
/// `--size` 是唯一的尺寸参数，含义随布局自适应：横版是窗口宽度（像素），
/// 竖版是窗口高度（像素）。另一个维度自动：横版高度由 `--lines` 行数决定，
/// 竖版宽度由 `--lines` 列数决定。
fn compute_geometry(cli: &Cli, screen_width: i32, screen_height: i32) -> Geometry {
    let (width, height) = if cli.vertical {
        let cell = LINE_HEIGHT as i32;
        let pad_x = PADDING_X as i32;
        let pad_y = PADDING_Y as i32;
        // 宽度自动：能容纳 `--lines` 列。
        let min_width = cell + 2 * pad_x;
        let max_width = (screen_width - 80).max(min_width);
        let width = (i32::from(cli.lines) * cell + 2 * pad_x).min(max_width);
        // 高度由 `--size` 指定，未指定时取屏幕高减上下边距。
        let min_height = cell + 2 * pad_y;
        let max_height = screen_height.max(min_height);
        let default_height = (screen_height - 2 * SCREEN_MARGIN).max(min_height);
        let height = match cli.size {
            Some(requested) => i32::from(requested).clamp(min_height, max_height),
            None => default_height.min(max_height),
        };
        (width, height)
    } else {
        let max_width = (screen_width - 80).max(320);
        let width = match cli.size {
            // 悬浮窗模式下 --size 表示窗口宽度（像素）；不传时默认占屏幕宽度的一半。
            Some(requested) => i32::from(requested).clamp(200, max_width),
            None => (screen_width / 2).clamp(200, max_width),
        };
        // 高度自动：`--lines` 行。
        let lines = i32::from(cli.lines);
        let height = lines * LINE_HEIGHT as i32 + 2 * PADDING_Y as i32;
        (width, height)
    };

    // 中性位置按布局解释：横版沿垂直轴贴边（水平居中），竖版沿水平轴贴边（垂直居中）。
    // 竖版 start 贴右、end 贴左：竖排从右往左读，start 应对应阅读起点一侧。
    let (x, y) = if cli.vertical {
        let y = ((screen_height - height) / 2).max(0);
        match cli.position {
            PositionArg::Start => ((screen_width - width - SCREEN_MARGIN).max(0), y),
            PositionArg::End => (SCREEN_MARGIN, y),
        }
    } else {
        let x = (screen_width - width) / 2;
        let y = match cli.position {
            PositionArg::Start => screen_height - height - SCREEN_MARGIN,
            PositionArg::End => SCREEN_MARGIN,
        };
        (x, y)
    };

    Geometry {
        width: width as u16,
        height: height as u16,
        x: x as i16,
        y: y as i16,
    }
}

struct WindowContext {
    connection: RustConnection,
    window: Window,
    gc: Gcontext,
    width: u16,
    height: u16,
    lsb_first: bool,
    vertical: bool,
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
        let geometry = compute_geometry(cli, screen_width, screen_height);
        let Geometry {
            width,
            height,
            x,
            y,
        } = geometry;

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
                "subview: overlay 窗口 id=0x{window:x} geometry={width}x{height}+{x}+{y} layout={} visual=0x{visual_id:x} lsb_first={lsb_first}",
                if cli.vertical {
                    "vertical"
                } else {
                    "horizontal"
                }
            );
        }

        Ok(Self {
            connection,
            window,
            gc,
            width,
            height,
            lsb_first,
            vertical: cli.vertical,
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
        let state = Arc::new(Mutex::new(CaptionState::new(crate::state::HISTORY_CAP)));
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
                let options = RenderOptions {
                    align,
                    fade,
                    background_alpha: cli.background_alpha,
                };
                self.render(&state, &options)?;
            }

            if eof.load(Ordering::Acquire) && !dirty.load(Ordering::Acquire) {
                let state = state.lock().map_err(|_| "字幕状态锁已中毒".to_string())?;
                let options = RenderOptions {
                    align,
                    fade,
                    background_alpha: cli.background_alpha,
                };
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
        let stream = compose_stream(state);
        if stream.text.is_empty() {
            return;
        }

        if self.vertical {
            draw_vertical_captions(
                frame,
                self.width,
                self.height,
                &stream,
                options,
                &mut self.font_system,
                &mut self.swash_cache,
            );
            return;
        }
        let text = stream.text.as_str();
        let partial_start = stream.partial_start;

        let width = i32::from(self.width);
        let height = i32::from(self.height);
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

        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
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
        buffer.shape_until_scroll(&mut self.font_system, false);

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

        // 清晰描边：白字在任何桌面背景上都有硬边，避免半透明背景发灰。
        let outline = outline_mask(&coverage, width as usize, height as usize, OUTLINE_RADIUS);
        blend_mask(frame, &outline, width, height, 0, 0, OUTLINE_ALPHA);

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

/// 竖排内容块的水平起点：列数不足一窗时整体贴右（竖排从右往左写，start 侧在右）。
fn vertical_block_start_x(width: i32, columns: usize, cell: f32) -> f32 {
    (width as f32 - PADDING_X - columns as f32 * cell).max(PADDING_X)
}

/// 竖版字幕：字从上到下、列从右到左（中文传统竖排）。
///
/// cosmic-text 只有横排排版，这里把每个 grapheme 单独排进自己的字框并居中，
/// 再按“最新列在最左”的顺序逐列绘制；列不足一窗时整体贴右（竖排 start 侧在右）。
/// 窗口高度决定每列字数，宽度决定列数。
fn draw_vertical_captions(
    frame: &mut [u32],
    width: u16,
    height: u16,
    stream: &ComposedStream,
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
    let columns =
        vertical::split_columns_tagged(&stream.text, capacity, max_columns, stream.partial_start);
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
            let (shift_x, shift_y) = vertical::punctuation_shift(grapheme)
                .map_or((0.0, 0.0), |(x, y)| (x * cell, y * cell));
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

/// 用可分离的最大值滤波把覆盖率向外扩张 `radius` 像素，生成清晰描边的遮罩。
fn outline_mask(mask: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
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

/// 组合后的文本流：整串文本 + 未定稿段起点。
struct ComposedStream {
    text: String,
    /// 未定稿段在 `text` 中的字节起点；`None` 表示没有 partial。
    partial_start: Option<usize>,
}

/// 把最近的字幕拼成一条连续文本流（每条字幕独立成行）。
///
/// 只保留尾部一段，既够折行显示最新几行，也避免长文本的排版开销。
fn compose_stream(state: &CaptionState) -> ComposedStream {
    const MAX_CHARS: usize = 400;

    let mut text = String::new();
    for caption in state.history() {
        push_caption(&mut text, caption);
    }
    let mut partial_start = None;
    if let Some(caption) = state.partial() {
        // 已有内容时，未定稿段前会先补一个换行。
        partial_start = Some(text.len() + usize::from(!text.is_empty()));
        push_caption(&mut text, caption);
    }
    if text.chars().count() > MAX_CHARS {
        let skip = text.chars().count() - MAX_CHARS;
        let skip_bytes = text
            .char_indices()
            .nth(skip)
            .map_or(text.len(), |(index, _)| index);
        text = text.chars().skip(skip).collect();
        // 尾段截断点可能落在换行上：去掉开头空行，避免多占一行高度。
        let trimmed = text.trim_start_matches(['\n', '\r']);
        let trim_bytes = text.len() - trimmed.len();
        text = trimmed.to_string();
        // 未定稿段起点随头部一起前移；被整个截掉时从头开始。
        let consumed = skip_bytes + trim_bytes;
        partial_start = partial_start.map(|start| start.saturating_sub(consumed));
    }
    // partial 没有可着色的内容（空串 / 被整体截掉）时视为没有未定稿段。
    if partial_start == Some(text.len()) {
        partial_start = None;
    }
    ComposedStream {
        text,
        partial_start,
    }
}

/// 追加一条字幕；不同字幕之间用换行分隔，各占独立行。
fn push_caption(text: &mut String, caption: &Caption) {
    if !text.is_empty() {
        text.push('\n');
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    fn geometry(args: &[&str], screen_width: i32, screen_height: i32) -> Geometry {
        let cli = Cli::try_parse_from(args).expect("参数应能解析");
        compute_geometry(&cli, screen_width, screen_height)
    }

    #[test]
    fn horizontal_default_width_is_half_screen() {
        // 1920x1080、默认 2 行：宽 = 屏幕宽一半（960px）、高 84px、水平居中、底部留 60px。
        let g = geometry(&["subview"], 1920, 1080);
        assert_eq!(g.width, 960);
        assert_eq!(g.height, 84);
        assert_eq!(g.x, 480);
        assert_eq!(g.y, 936);
    }

    #[test]
    fn horizontal_end_position_is_top() {
        let g = geometry(&["subview", "--position", "end"], 1920, 1080);
        assert_eq!(g.x, 480);
        assert_eq!(g.y, 60);
    }

    #[test]
    fn horizontal_size_sets_window_width_only() {
        // 横版：--size 是宽度，高度仍由 --lines 自动算。
        let g = geometry(&["subview", "--size", "600"], 1920, 1080);
        assert_eq!(g.width, 600);
        assert_eq!(g.height, 84);
        assert_eq!(g.x, 660);
        assert_eq!(g.y, 936);
    }

    #[test]
    fn horizontal_size_is_clamped() {
        let g = geometry(&["subview", "--size", "10"], 1920, 1080);
        assert_eq!(g.width, 200);
        let g = geometry(&["subview", "--size", "5000"], 1920, 1080);
        assert_eq!(g.width, 1840);
    }

    #[test]
    fn vertical_defaults_use_lines_for_width() {
        let g = geometry(&["subview", "--vertical"], 1920, 1080);
        // 2 列 × 32px + 左右内边距 16px。
        assert_eq!(g.width, 96);
        // 屏幕高减上下边距 60px。
        assert_eq!(g.height, 960);
        // 默认 start：竖版贴右，垂直居中。
        assert_eq!(g.x, 1764);
        assert_eq!(g.y, 60);
    }

    #[test]
    fn vertical_size_sets_window_height_only() {
        // 竖版：--size 是高度，宽度仍由 --lines 自动算。
        let g = geometry(&["subview", "--vertical", "--size", "800"], 1920, 1080);
        assert_eq!(g.width, 96);
        assert_eq!(g.height, 800);
        assert_eq!(g.x, 1764);
        assert_eq!(g.y, 140);
    }

    #[test]
    fn vertical_size_is_clamped() {
        // 高度下限能放下一个字框。
        let g = geometry(&["subview", "--vertical", "--size", "10"], 1920, 1080);
        assert_eq!(g.height, 52);
        let g = geometry(&["subview", "--vertical", "--size", "5000"], 1920, 1080);
        assert_eq!(g.height, 1080);
        assert_eq!(g.y, 0);
    }

    #[test]
    fn vertical_end_position_is_left() {
        let g = geometry(
            &[
                "subview",
                "--vertical",
                "--position",
                "end",
                "--size",
                "800",
            ],
            1920,
            1080,
        );
        // 贴左边缘，垂直居中。
        assert_eq!(g.x, 60);
        assert_eq!(g.y, 140);
    }

    #[test]
    fn vertical_columns_hug_right_when_not_full() {
        // 96px 两列窗口：只有一列时贴右（48px），满列时从左右内边距之间铺满（16px）。
        assert_eq!(vertical_block_start_x(96, 1, LINE_HEIGHT), 48.0);
        assert_eq!(vertical_block_start_x(96, 2, LINE_HEIGHT), 16.0);
    }

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

    #[test]
    fn compose_stream_puts_each_caption_on_its_own_line() {
        let mut state = CaptionState::new(crate::state::HISTORY_CAP);
        for text in ["一", "二"] {
            state.apply(crate::protocol::Event::Final(Caption {
                text: text.to_string(),
                speaker: None,
            }));
        }
        state.apply(crate::protocol::Event::Partial(Caption {
            text: "三".to_string(),
            speaker: None,
        }));
        let stream = compose_stream(&state);
        assert_eq!(stream.text, "一\n二\n三");
        assert_eq!(stream.partial_start, Some("一\n二\n".len()));
    }

    #[test]
    fn compose_stream_without_partial_has_no_split() {
        let mut state = CaptionState::new(crate::state::HISTORY_CAP);
        state.apply(crate::protocol::Event::Final(Caption {
            text: "一".to_string(),
            speaker: None,
        }));
        let stream = compose_stream(&state);
        assert_eq!(stream.text, "一");
        assert_eq!(stream.partial_start, None);
    }

    #[test]
    fn compose_stream_keeps_partial_start_after_truncation() {
        let mut state = CaptionState::new(crate::state::HISTORY_CAP);
        let long = "b".repeat(399);
        state.apply(crate::protocol::Event::Final(Caption {
            text: long.clone(),
            speaker: None,
        }));
        state.apply(crate::protocol::Event::Partial(Caption {
            text: "三".to_string(),
            speaker: None,
        }));
        // 总长 401 字符：截掉 1 个字符后剩 398 个 b + 换行 + 未定稿的“三”。
        let stream = compose_stream(&state);
        assert_eq!(stream.text, format!("{}\n三", "b".repeat(398)));
        assert_eq!(stream.partial_start, Some(399));
    }

    #[test]
    fn compose_stream_drops_blank_line_left_by_truncation() {
        let mut state = CaptionState::new(crate::state::HISTORY_CAP);
        state.apply(crate::protocol::Event::Final(Caption {
            text: "x".to_string(),
            speaker: None,
        }));
        let long = "b".repeat(399);
        state.apply(crate::protocol::Event::Final(Caption {
            text: long.clone(),
            speaker: None,
        }));
        // 总长 401 字符，截掉第 1 个字符后恰好以换行开头，应去掉这个空行。
        let stream = compose_stream(&state);
        assert_eq!(stream.text, long);
        assert_eq!(stream.partial_start, None);
    }
}
