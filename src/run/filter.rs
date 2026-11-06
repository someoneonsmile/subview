//! 纯文本过滤器：输出到非终端时只写出定稿字幕行。

use std::io::{self, Read, Write};

use crate::config::ReaderOptions;
use crate::protocol::{Event, Parser, Warning};
use crate::signal;
use crate::state::speaker_prefix;

/// 运行纯文本过滤器，返回进程退出码。
pub fn run(options: ReaderOptions) -> i32 {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let mut parser = Parser::new(options.format, options.policy.strip_control);
    let mut events: Vec<Event> = Vec::new();
    let mut warnings: Vec<Warning> = Vec::new();
    let mut buffer = [0u8; 8192];

    loop {
        if signal::stop_requested() {
            break;
        }
        match reader.read(&mut buffer) {
            Ok(0) => {
                events.clear();
                parser.finish(&mut events);
                report_warnings(&mut parser, &mut warnings);
                write_final_lines(&mut out, &events);
                if let Err(error) = out.flush() {
                    eprintln!("subview: {error}");
                    return 1;
                }
                break;
            }
            Ok(read) => {
                events.clear();
                parser.feed(&buffer[..read], &mut events);
                report_warnings(&mut parser, &mut warnings);
                write_final_lines(&mut out, &events);
                if let Err(error) = out.flush() {
                    eprintln!("subview: {error}");
                    return 1;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                eprintln!("subview: {error}");
                return 1;
            }
        }
    }

    signal::exit_code()
}

/// 把解析器累积的警告输出到 stderr。
fn report_warnings(parser: &mut Parser, warnings: &mut Vec<Warning>) {
    parser.drain_warnings(warnings);
    for warning in warnings.drain(..) {
        eprintln!("subview: warning: {warning}");
    }
}

fn write_final_lines<W: Write>(out: &mut W, events: &[Event]) {
    for event in events {
        if let Event::Final(caption) = event {
            if let Some(speaker) = &caption.speaker {
                let _ = write!(out, "{}", speaker_prefix(speaker));
            }
            let _ = writeln!(out, "{}", caption.text);
        }
    }
}
