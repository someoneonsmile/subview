//! 从标准输入读取字幕流：解析、写入共享状态、通知消费者。

use std::io::{self, Read};
use std::sync::{Arc, Mutex};

use crate::protocol::{Event, InputFormat, Parser};
use crate::state::CaptionState;

/// 读线程产生的事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderEvent {
    /// 状态已更新，可能需要重绘。
    Updated,
    /// 标准输入已结束，读线程退出。
    Finished,
}

/// 启动读线程。
///
/// `notify` 在状态更新后调用，必须足够轻量（不能阻塞读线程）。
pub fn spawn_reader<F>(
    state: Arc<Mutex<CaptionState>>,
    format: InputFormat,
    strip_control: bool,
    notify: F,
) where
    F: Fn(ReaderEvent) + Send + 'static,
{
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        let mut parser = Parser::new(format, strip_control);
        let mut events: Vec<Event> = Vec::new();
        let mut buffer = [0u8; 8192];

        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    events.clear();
                    parser.finish(&mut events);
                    apply(&state, &events);
                    notify(ReaderEvent::Finished);
                    break;
                }
                Ok(read) => {
                    events.clear();
                    parser.feed(&buffer[..read], &mut events);
                    apply(&state, &events);
                    notify(ReaderEvent::Updated);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    notify(ReaderEvent::Finished);
                    break;
                }
            }
        }
    });
}

fn apply(state: &Mutex<CaptionState>, events: &[Event]) {
    if events.is_empty() {
        return;
    }
    if let Ok(mut state) = state.lock() {
        for event in events {
            state.apply(event.clone());
        }
    }
}
