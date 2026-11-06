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

mod canvas;
mod compose;
mod horizontal;
mod vertical;
mod window;

use std::io;

use cosmic_text::{Color, FontSystem, SwashCache};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    ConnectionExt as XprotoConnectionExt, Gcontext, ImageFormat, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::config::{OverlayConfig, ReaderOptions};
use crate::render::layout::Align;
use crate::render::{Fade, Poll, Renderer};
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
/// 悬浮窗启动结果。
pub enum Outcome {
    /// 正常退出，携带进程退出码。
    Exited(i32),
    /// 图形环境不可用，调用方可以回退到终端界面。
    Unavailable(String),
}

/// 启动悬浮窗后端；主循环由 [`crate::run::drive`] 接管。
pub fn run(config: &OverlayConfig, options: ReaderOptions) -> Outcome {
    let mut backend = match OverlayBackend::new(config) {
        Ok(backend) => backend,
        Err(error) => return Outcome::Unavailable(error),
    };
    let input = crate::run::spawn_input(options);
    match crate::run::drive(
        &mut backend,
        &input.state,
        &input.dirty,
        &input.eof,
        config.linger,
    ) {
        Ok(()) => Outcome::Exited(signal::exit_code()),
        Err(error) => {
            eprintln!("subview: 悬浮窗渲染失败：{error}");
            Outcome::Exited(1)
        }
    }
}

impl Renderer for OverlayBackend {
    fn poll(&mut self) -> io::Result<Poll> {
        let mut result = Poll::Idle;
        loop {
            match self.connection.poll_for_event() {
                Ok(Some(Event::Expose(expose))) => {
                    if expose.count == 0 {
                        result = Poll::Redraw;
                    }
                }
                Ok(Some(Event::DestroyNotify(_))) => return Ok(Poll::Closed),
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(error) => return Err(io::Error::other(error.to_string())),
            }
        }
        Ok(result)
    }

    fn draw(&mut self, state: &CaptionState, fade: Fade) -> io::Result<()> {
        let options = RenderOptions {
            align: self.align,
            fade: fade.percent(),
            background_alpha: self.background_alpha,
        };
        self.render(state, &options)
    }
}

impl Drop for OverlayBackend {
    fn drop(&mut self) {
        let _ = self.connection.destroy_window(self.window);
        let _ = self.connection.flush();
    }
}

/// 每帧的渲染选项。
struct RenderOptions {
    align: Align,
    fade: u8,
    /// 背景条不透明度（0–255，淡出时按进度递减）。
    background_alpha: u8,
}

struct OverlayBackend {
    connection: RustConnection,
    window: Window,
    gc: Gcontext,
    width: u16,
    height: u16,
    lsb_first: bool,
    vertical: bool,
    align: Align,
    background_alpha: u8,
    font_system: FontSystem,
    swash_cache: SwashCache,
}

impl OverlayBackend {
    fn render(&mut self, state: &CaptionState, options: &RenderOptions) -> io::Result<()> {
        let width = self.width;
        let height = self.height;
        let mut frame = vec![0u32; width as usize * height as usize];
        self.draw_captions(&mut frame, state, options);
        let bytes = canvas::pixels_to_bytes(&frame, self.lsb_first);
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
            .map_err(|error| io::Error::other(format!("PutImage 失败：{error}")))?;
        self.connection
            .flush()
            .map_err(|error| io::Error::other(format!("X11 刷新失败：{error}")))
    }

    fn draw_captions(&mut self, frame: &mut [u32], state: &CaptionState, options: &RenderOptions) {
        let stream = compose::compose(state);
        if stream.is_empty() {
            return;
        }
        if self.vertical {
            vertical::draw(
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
        horizontal::draw(
            frame,
            self.width,
            self.height,
            &stream,
            options,
            &mut self.font_system,
            &mut self.swash_cache,
        );
    }
}
