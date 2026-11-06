//! 通用渲染流程：读线程、帧循环与 linger 淡出状态机。
//!
//! 终端后端与悬浮窗后端共用这条流程；后端只负责实现 [`Renderer`]。

pub mod filter;
pub mod terminal;

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::ReaderOptions;
use crate::reader;
use crate::render::{Fade, Poll, Renderer};
use crate::signal;
use crate::state::CaptionState;

/// 主循环轮询间隔。
pub const TICK: Duration = Duration::from_millis(16);
/// linger 触发后的淡出时长（秒）。
pub const FADE_SECS: f64 = 0.8;

/// 共享输入：读线程通过 dirty / eof 标志通知主循环。
pub struct Input {
    pub state: Arc<Mutex<CaptionState>>,
    pub dirty: Arc<AtomicBool>,
    pub eof: Arc<AtomicBool>,
}

/// 启动读线程，返回共享输入句柄。
pub fn spawn_input(options: ReaderOptions) -> Input {
    let state = Arc::new(Mutex::new(CaptionState::new(crate::state::HISTORY_CAP)));
    let dirty = Arc::new(AtomicBool::new(false));
    let eof = Arc::new(AtomicBool::new(false));
    let dirty_reader = dirty.clone();
    let eof_reader = eof.clone();
    reader::spawn_reader(state.clone(), options, move |event| {
        dirty_reader.store(true, Ordering::Release);
        if event == reader::ReaderEvent::Finished {
            eof_reader.store(true, Ordering::Release);
        }
    });
    Input { state, dirty, eof }
}

/// linger 淡出状态机。
pub struct Linger {
    enabled: bool,
    last_event: Instant,
    fade: Fade,
    done: bool,
}

impl Linger {
    pub fn new(linger: f64) -> Self {
        Self::at(linger, Instant::now())
    }

    /// 指定起始时刻（便于测试注入时间）。
    pub fn at(linger: f64, now: Instant) -> Self {
        Self {
            enabled: linger > 0.0,
            last_event: now,
            fade: Fade::NONE,
            done: linger <= 0.0,
        }
    }

    /// 收到新内容：重置淡出计时。
    pub fn on_event(&mut self) {
        self.on_event_at(Instant::now());
    }

    pub fn on_event_at(&mut self, now: Instant) {
        self.last_event = now;
        self.fade = Fade::NONE;
        self.done = !self.enabled;
    }

    /// 推进淡出。返回 `Some` 表示淡出进度变化、需要重绘；`None` 表示无变化。
    pub fn tick(&mut self, linger: f64, now: Instant) -> Option<Fade> {
        if self.done || !self.enabled {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.last_event).as_secs_f64();
        if elapsed <= linger {
            return None;
        }
        let progress = ((elapsed - linger) / FADE_SECS).clamp(0.0, 1.0);
        let fade = Fade::new((progress * 100.0).round() as u8);
        if fade == self.fade {
            return None;
        }
        self.fade = fade;
        if progress >= 1.0 {
            self.done = true;
        }
        Some(fade)
    }

    /// 当前淡出进度。
    pub fn fade(&self) -> Fade {
        self.fade
    }

    /// 淡出是否已结束（调用方据此清空显示）。
    pub fn is_done(&self) -> bool {
        self.done
    }
}

/// 主循环：拉取后端事件、按需重绘、处理 linger 淡出与 EOF。
pub fn drive<R: Renderer>(
    renderer: &mut R,
    state: &Arc<Mutex<CaptionState>>,
    dirty: &AtomicBool,
    eof: &AtomicBool,
    linger_secs: f64,
) -> io::Result<()> {
    let mut linger = Linger::new(linger_secs);
    let mut needs_redraw = false;

    loop {
        if signal::stop_requested() {
            return Ok(());
        }

        match renderer.poll()? {
            Poll::Redraw => needs_redraw = true,
            Poll::Closed => return Ok(()),
            Poll::Idle => {}
        }

        let changed = dirty.swap(false, Ordering::AcqRel);
        if changed {
            linger.on_event();
        }

        if linger.tick(linger_secs, Instant::now()).is_some() {
            if linger.is_done() {
                lock_state(state)?.clear_display();
            }
            needs_redraw = true;
        }

        if needs_redraw || changed {
            needs_redraw = false;
            let state = lock_state(state)?;
            renderer.draw(&state, linger.fade())?;
        }

        if eof.load(Ordering::Acquire) && !dirty.load(Ordering::Acquire) {
            let state = lock_state(state)?;
            renderer.draw(&state, linger.fade())?;
            return Ok(());
        }

        thread::sleep(TICK);
    }
}

fn lock_state(state: &Arc<Mutex<CaptionState>>) -> io::Result<MutexGuard<'_, CaptionState>> {
    state
        .lock()
        .map_err(|_| io::Error::other("字幕状态锁已中毒"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Caption, Event};
    use crate::render::Poll;

    #[derive(Default)]
    struct FakeRenderer {
        draws: usize,
        polls: usize,
    }

    impl Renderer for FakeRenderer {
        fn poll(&mut self) -> io::Result<Poll> {
            self.polls += 1;
            Ok(Poll::Idle)
        }

        fn draw(&mut self, _state: &CaptionState, _fade: Fade) -> io::Result<()> {
            self.draws += 1;
            Ok(())
        }
    }

    fn caption(text: &str) -> Caption {
        Caption {
            text: text.to_string(),
            speaker: None,
        }
    }

    #[test]
    fn linger_disabled_never_fades() {
        let t0 = Instant::now();
        let mut linger = Linger::at(0.0, t0);
        assert!(linger.tick(0.0, t0 + Duration::from_secs(10)).is_none());
        // 未启用 linger 时视为“已完成”，主循环不会清屏。
        assert!(linger.is_done());
        assert_eq!(linger.fade(), Fade::NONE);
    }

    #[test]
    fn linger_fades_after_threshold() {
        let t0 = Instant::now();
        let mut linger = Linger::at(2.0, t0);
        // 阈值内不淡出。
        assert!(linger.tick(2.0, t0 + Duration::from_millis(1500)).is_none());
        assert_eq!(linger.fade(), Fade::NONE);
        // 越过阈值后开始淡出。
        let fade = linger
            .tick(2.0, t0 + Duration::from_millis(2400))
            .expect("应开始淡出");
        assert!(fade.percent() > 0 && fade.percent() < 100);
    }

    #[test]
    fn linger_reaches_done() {
        let t0 = Instant::now();
        let mut linger = Linger::at(1.0, t0);
        assert!(
            linger
                .tick(1.0, t0 + Duration::from_secs(3))
                .is_some_and(|fade| fade == Fade::FULL)
        );
        assert!(linger.is_done());
        // 结束后不再产生变化。
        assert!(linger.tick(1.0, t0 + Duration::from_secs(5)).is_none());
    }

    #[test]
    fn new_event_resets_fade() {
        let t0 = Instant::now();
        let mut linger = Linger::at(1.0, t0);
        assert!(linger.tick(1.0, t0 + Duration::from_secs(3)).is_some());
        assert!(linger.is_done());

        linger.on_event_at(t0 + Duration::from_secs(4));
        assert_eq!(linger.fade(), Fade::NONE);
        assert!(!linger.is_done());
        assert!(
            linger
                .tick(
                    1.0,
                    t0 + Duration::from_secs(4) + Duration::from_millis(500)
                )
                .is_none()
        );
    }

    #[test]
    fn drive_draws_before_exit_on_eof() {
        let state = Arc::new(Mutex::new(CaptionState::new(4)));
        state.lock().unwrap().apply(Event::Final(caption("一")));
        let dirty = Arc::new(AtomicBool::new(false));
        let eof = Arc::new(AtomicBool::new(true));
        let mut renderer = FakeRenderer::default();

        drive(&mut renderer, &state, &dirty, &eof, 0.0).expect("主循环应正常退出");

        assert_eq!(renderer.draws, 1, "EOF 时应补最后一帧");
        assert!(renderer.polls >= 1);
    }

    #[test]
    fn drive_draws_on_dirty_and_eof() {
        let state = Arc::new(Mutex::new(CaptionState::new(4)));
        let dirty = Arc::new(AtomicBool::new(true));
        let eof = Arc::new(AtomicBool::new(true));
        let mut renderer = FakeRenderer::default();

        drive(&mut renderer, &state, &dirty, &eof, 0.0).expect("主循环应正常退出");

        assert_eq!(renderer.draws, 2, "dirty 一帧 + EOF 补一帧");
    }
}
