//! X11 资源与几何：连接、窗口、visual 选择与 compositor 检测。

use cosmic_text::{FontSystem, SwashCache};
use x11rb::connection::Connection;
use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
use x11rb::protocol::xproto::{
    ClipOrdering, ColormapAlloc, ConnectionExt as XprotoConnectionExt, CreateGCAux,
    CreateWindowAux, EventMask, ImageOrder, PropMode, Screen, VisualClass, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

use crate::config::{OverlayConfig, Position};

use super::*;

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
fn compute_geometry(config: &OverlayConfig, screen_width: i32, screen_height: i32) -> Geometry {
    let (width, height) = if config.vertical {
        let cell = LINE_HEIGHT as i32;
        let pad_x = PADDING_X as i32;
        let pad_y = PADDING_Y as i32;
        // 宽度自动：能容纳 `--lines` 列。
        let min_width = cell + 2 * pad_x;
        let max_width = (screen_width - 80).max(min_width);
        let width = (i32::from(config.lines) * cell + 2 * pad_x).min(max_width);
        // 高度由 `--size` 指定，未指定时取屏幕高减上下边距。
        let min_height = cell + 2 * pad_y;
        let max_height = screen_height.max(min_height);
        let default_height = (screen_height - 2 * SCREEN_MARGIN).max(min_height);
        let height = match config.size {
            Some(requested) => i32::from(requested).clamp(min_height, max_height),
            None => default_height.min(max_height),
        };
        (width, height)
    } else {
        let max_width = (screen_width - 80).max(320);
        let width = match config.size {
            // 悬浮窗模式下 --size 表示窗口宽度（像素）；不传时默认占屏幕宽度的一半。
            Some(requested) => i32::from(requested).clamp(200, max_width),
            None => (screen_width / 2).clamp(200, max_width),
        };
        // 高度自动：`--lines` 行。
        let lines = i32::from(config.lines);
        let height = lines * LINE_HEIGHT as i32 + 2 * PADDING_Y as i32;
        (width, height)
    };

    // 中性位置按布局解释：横版沿垂直轴贴边（水平居中），竖版沿水平轴贴边（垂直居中）。
    // 竖版 start 贴右、end 贴左：竖排从右往左读，start 应对应阅读起点一侧。
    let (x, y) = if config.vertical {
        let y = ((screen_height - height) / 2).max(0);
        match config.position {
            Position::Start => ((screen_width - width - SCREEN_MARGIN).max(0), y),
            Position::End => (SCREEN_MARGIN, y),
        }
    } else {
        let x = (screen_width - width) / 2;
        let y = match config.position {
            Position::Start => screen_height - height - SCREEN_MARGIN,
            Position::End => SCREEN_MARGIN,
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

impl OverlayBackend {
    pub(super) fn new(config: &OverlayConfig) -> Result<Self, String> {
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
        let geometry = compute_geometry(config, screen_width, screen_height);
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

        if config.debug {
            crate::ui::note(format!(
                "overlay 窗口 id=0x{window:x} geometry={width}x{height}+{x}+{y} layout={} visual=0x{visual_id:x} lsb_first={lsb_first}",
                if config.vertical {
                    "vertical"
                } else {
                    "horizontal"
                }
            ));
        }

        Ok(Self {
            connection,
            window,
            gc,
            width,
            height,
            lsb_first,
            vertical: config.vertical,
            align: config.align,
            background_alpha: config.background_alpha,
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
        })
    }
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
        crate::ui::warn("未检测到合成器（compositor），透明背景可能显示为黑色");
        crate::ui::note("提示：启动 picom / mutter / kwin 等合成器以启用透明");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    use crate::cli::Cli;

    fn geometry(args: &[&str], screen_width: i32, screen_height: i32) -> Geometry {
        let cli = Cli::try_parse_from(args).expect("参数应能解析");
        compute_geometry(&OverlayConfig::from(&cli), screen_width, screen_height)
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
}
