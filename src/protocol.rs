//! 输入协议：把标准输入的字节流解析为字幕事件。
//!
//! # `plain` 格式（默认）
//!
//! 借用终端自身的行语义：
//!
//! - `\n`：提交（final），此前累积的内容成为一条定稿字幕
//! - `\r`：覆盖（partial），此前累积的内容作为中间结果展示、可被丢弃
//! - 尚未由 `\r` / `\n` 结束的行内容也会作为 partial 实时显示
//! - 空行：清屏
//! - `\r\n`（CRLF）：视为一次行结束
//!
//! # `jsonl` 格式
//!
//! 每行一个 JSON 对象：
//!
//! ```text
//! {"event":"partial","text":"欢迎"}
//! {"event":"final","text":"欢迎收听本期节目","speaker":"A"}
//! {"event":"clear"}
//! ```
//!
//! 显示模式会剥除文本中的控制字符（`\t` 除外）防止终端转义注入；作为纯文本
//! 过滤器（`--raw` 输出到非终端）时由调用方决定是否原样透传。

use std::time::{Duration, Instant};

/// 一条字幕。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Caption {
    pub text: String,
    pub speaker: Option<String>,
}

/// 解析器产生的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// 中间结果：覆盖当前未定稿行；渲染跟不上时可以丢弃。
    Partial(Caption),
    /// 定稿：追加到字幕历史；不可丢弃。
    Final(Caption),
    /// 清空字幕区。
    Clear,
}

/// 输入格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFormat {
    Plain,
    Jsonl,
}

/// 单行最大字节数；超出后只保留尾部，避免无界内存。
const MAX_LINE_BYTES: usize = 64 * 1024;
/// 超长行截断后保留的尾部字节数。
const TAIL_BYTES: usize = 32 * 1024;
/// 告警限流间隔。
const WARN_INTERVAL: Duration = Duration::from_secs(5);

/// 解析过程中产生的警告。
///
/// 解析器只记录、不输出；由调用方（读线程 / 纯文本过滤器）决定写到哪、用什么格式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// plain 行超过上限，只保留尾部。
    LineTruncated { limit_kib: usize },
    /// jsonl 行超过上限，已丢弃。
    JsonLineTooLong { limit_kib: usize },
    /// JSON 行无法解析。
    JsonParseFailed,
    /// partial / final 事件缺少 text 字段。
    JsonMissingText { event: String },
    /// 未知事件类型。
    JsonUnknownEvent { event: String },
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LineTruncated { limit_kib } => {
                write!(f, "输入行超过 {limit_kib} KiB，只保留尾部")
            }
            Self::JsonLineTooLong { limit_kib } => {
                write!(f, "JSON 行超过 {limit_kib} KiB，已丢弃")
            }
            Self::JsonParseFailed => f.write_str("无法解析 JSON 行，已忽略"),
            Self::JsonMissingText { event } => write!(f, "{event} 事件缺少 text 字段"),
            Self::JsonUnknownEvent { event } => write!(f, "未知事件类型 {event:?}，已忽略"),
        }
    }
}

/// 流式协议解析器。
pub enum Parser {
    Plain(PlainParser),
    Jsonl(JsonlParser),
}

impl Parser {
    pub fn new(format: InputFormat, strip_control: bool) -> Self {
        match format {
            InputFormat::Plain => Self::Plain(PlainParser::new(strip_control)),
            InputFormat::Jsonl => Self::Jsonl(JsonlParser::new(strip_control)),
        }
    }

    /// 消费一段输入字节，把完整事件追加到 `out`。
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Event>) {
        match self {
            Self::Plain(parser) => parser.feed(bytes, out),
            Self::Jsonl(parser) => parser.feed(bytes, out),
        }
    }

    /// 输入结束（EOF）：把未提交的内容定稿。
    pub fn finish(&mut self, out: &mut Vec<Event>) {
        match self {
            Self::Plain(parser) => parser.finish(out),
            Self::Jsonl(parser) => parser.finish(out),
        }
    }

    /// 把还没被 `\r` / `\n` 结束的行内容作为 partial 发出。
    ///
    /// 让逐字节 / 逐块写入的输入也能实时上屏；jsonl 以换行分帧，没有
    /// “未结束行”的语义，不产生事件。
    pub fn flush_pending(&mut self, out: &mut Vec<Event>) {
        match self {
            Self::Plain(parser) => parser.flush_pending(out),
            Self::Jsonl(_) => {}
        }
    }

    /// 取走累积的警告，交给调用方输出。
    pub fn drain_warnings(&mut self, out: &mut Vec<Warning>) {
        match self {
            Self::Plain(parser) => parser.drain_warnings(out),
            Self::Jsonl(parser) => parser.drain_warnings(out),
        }
    }
}

// ---------------------------------------------------------------- plain

/// `plain` 格式的增量解析器。
pub struct PlainParser {
    buf: Vec<u8>,
    last_partial: Option<Caption>,
    last_was_cr: bool,
    strip_control: bool,
    warned_truncate: bool,
    warnings: Vec<Warning>,
}

impl PlainParser {
    /// 取走累积的警告。
    pub fn drain_warnings(&mut self, out: &mut Vec<Warning>) {
        out.append(&mut self.warnings);
    }

    pub fn new(strip_control: bool) -> Self {
        Self {
            buf: Vec::new(),
            last_partial: None,
            last_was_cr: false,
            strip_control,
            warned_truncate: false,
            warnings: Vec::new(),
        }
    }

    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Event>) {
        for &byte in bytes {
            match byte {
                b'\r' => self.on_cr(out),
                b'\n' => self.on_lf(out),
                _ => {
                    self.last_was_cr = false;
                    self.buf.push(byte);
                    if self.buf.len() > MAX_LINE_BYTES {
                        if !self.warned_truncate {
                            self.warnings.push(Warning::LineTruncated {
                                limit_kib: MAX_LINE_BYTES / 1024,
                            });
                            self.warned_truncate = true;
                        }
                        let cut = self.buf.len() - TAIL_BYTES;
                        self.buf.drain(..cut);
                    }
                }
            }
        }
    }

    fn on_cr(&mut self, out: &mut Vec<Event>) {
        self.last_was_cr = true;
        if let Some(caption) = self.take_caption() {
            self.last_partial = Some(caption.clone());
            out.push(Event::Partial(caption));
        }
    }

    fn on_lf(&mut self, out: &mut Vec<Event>) {
        // CRLF：`\r` 已经把内容作为 partial 发出，这里把它定稿。
        if self.last_was_cr && self.buf.is_empty() {
            self.last_was_cr = false;
            if let Some(caption) = self.last_partial.take() {
                out.push(Event::Final(caption));
            }
            return;
        }
        self.last_was_cr = false;
        self.last_partial = None;
        match self.take_caption() {
            Some(caption) => out.push(Event::Final(caption)),
            None => out.push(Event::Clear),
        }
    }

    fn take_caption(&mut self) -> Option<Caption> {
        if self.buf.is_empty() {
            return None;
        }
        let raw = std::mem::take(&mut self.buf);
        let text = decode_text(&raw, self.strip_control);
        if text.is_empty() {
            return None;
        }
        Some(Caption {
            text,
            speaker: None,
        })
    }

    pub fn finish(&mut self, out: &mut Vec<Event>) {
        if let Some(caption) = self.take_caption() {
            out.push(Event::Final(caption));
        } else if let Some(caption) = self.last_partial.take() {
            out.push(Event::Final(caption));
        }
    }

    /// 把尚未结束的行内容作为 partial 发出（实时预览）。
    pub fn flush_pending(&mut self, out: &mut Vec<Event>) {
        if let Some(caption) = self.pending_caption() {
            out.push(Event::Partial(caption));
        }
    }

    /// 缓冲区中尚未结束的内容；只解码完整的 UTF-8 前缀。
    fn pending_caption(&self) -> Option<Caption> {
        // 跨 `read` 的半个字符先不显示，避免闪一下 U+FFFD。
        let bytes = match std::str::from_utf8(&self.buf) {
            Ok(_) => self.buf.as_slice(),
            Err(error) => &self.buf[..error.valid_up_to()],
        };
        let text = decode_text(bytes, self.strip_control);
        if text.trim().is_empty() {
            return None;
        }
        Some(Caption {
            text,
            speaker: None,
        })
    }
}

/// 字节 → 文本；非法 UTF-8 替换为 U+FFFD。
fn decode_text(bytes: &[u8], strip_control: bool) -> String {
    let decoded = String::from_utf8_lossy(bytes);
    if strip_control {
        sanitize_text(&decoded, true)
    } else {
        decoded.into_owned()
    }
}

/// 剥除控制字符（`\t` 保留），防止转义序列注入终端。
fn sanitize_text(text: &str, strip_control: bool) -> String {
    if !strip_control {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '\t' || !ch.is_control() {
            out.push(ch);
        }
    }
    out
}

// ---------------------------------------------------------------- jsonl

/// `jsonl` 格式的增量解析器。
pub struct JsonlParser {
    buf: Vec<u8>,
    skipping_long_line: bool,
    strip_control: bool,
    warned_at: Option<Instant>,
    warned_long_line: bool,
    warnings: Vec<Warning>,
}

#[derive(serde::Deserialize)]
struct WireEvent {
    event: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    speaker: Option<String>,
}

impl JsonlParser {
    /// 取走累积的警告。
    pub fn drain_warnings(&mut self, out: &mut Vec<Warning>) {
        out.append(&mut self.warnings);
    }

    pub fn new(strip_control: bool) -> Self {
        Self {
            buf: Vec::new(),
            skipping_long_line: false,
            strip_control,
            warned_at: None,
            warned_long_line: false,
            warnings: Vec::new(),
        }
    }

    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Event>) {
        for &byte in bytes {
            if self.skipping_long_line {
                if byte == b'\n' {
                    self.skipping_long_line = false;
                }
                continue;
            }
            if byte == b'\n' {
                let mut line = std::mem::take(&mut self.buf);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.handle_line(&line, out);
            } else {
                self.buf.push(byte);
                if self.buf.len() > MAX_LINE_BYTES {
                    if !self.warned_long_line {
                        self.warnings.push(Warning::JsonLineTooLong {
                            limit_kib: MAX_LINE_BYTES / 1024,
                        });
                        self.warned_long_line = true;
                    }
                    self.buf.clear();
                    self.skipping_long_line = true;
                }
            }
        }
    }

    fn handle_line(&mut self, line: &[u8], out: &mut Vec<Event>) {
        if line.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(line);
        let event = match serde_json::from_str::<WireEvent>(&text) {
            Ok(event) => event,
            Err(_) => {
                self.push_warning(Warning::JsonParseFailed);
                return;
            }
        };
        match event.event.as_str() {
            "partial" | "final" => {
                let Some(text) = event.text else {
                    self.push_warning(Warning::JsonMissingText {
                        event: event.event.clone(),
                    });
                    return;
                };
                let text = sanitize_text(&text, self.strip_control);
                if text.is_empty() {
                    return;
                }
                let caption = Caption {
                    text,
                    speaker: event.speaker,
                };
                if event.event == "partial" {
                    out.push(Event::Partial(caption));
                } else {
                    out.push(Event::Final(caption));
                }
            }
            "clear" => out.push(Event::Clear),
            other => self.push_warning(Warning::JsonUnknownEvent {
                event: other.to_string(),
            }),
        }
    }

    fn push_warning(&mut self, warning: Warning) {
        let now = Instant::now();
        let allowed = match self.warned_at {
            Some(last) => now.duration_since(last) >= WARN_INTERVAL,
            None => true,
        };
        if allowed {
            self.warnings.push(warning);
            self.warned_at = Some(now);
        }
    }

    pub fn finish(&mut self, out: &mut Vec<Event>) {
        if self.buf.is_empty() {
            return;
        }
        let mut line = std::mem::take(&mut self.buf);
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        self.handle_line(&line, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_plain(chunks: &[&[u8]], finish: bool) -> Vec<Event> {
        let mut parser = PlainParser::new(true);
        let mut out = Vec::new();
        for chunk in chunks {
            parser.feed(chunk, &mut out);
        }
        if finish {
            parser.finish(&mut out);
        }
        out
    }

    fn caption(text: &str) -> Caption {
        Caption {
            text: text.to_string(),
            speaker: None,
        }
    }

    #[test]
    fn plain_final() {
        assert_eq!(
            parse_plain(&[b"hello\n"], true),
            vec![Event::Final(caption("hello"))]
        );
    }

    #[test]
    fn plain_partial() {
        assert_eq!(
            parse_plain(&[b"hel\r"], true),
            vec![Event::Partial(caption("hel")), Event::Final(caption("hel")),]
        );
    }

    #[test]
    fn plain_crlf_is_one_line() {
        assert_eq!(
            parse_plain(&[b"hello\r\n"], true),
            vec![
                Event::Partial(caption("hello")),
                Event::Final(caption("hello")),
            ]
        );
    }

    #[test]
    fn plain_partial_then_final() {
        assert_eq!(
            parse_plain(
                &[b"\xe4\xbd\xa0\xe5\xa5\xbd\r\xe4\xb8\x96\xe7\x95\x8c\n"],
                true
            ),
            vec![
                Event::Partial(caption("你好")),
                Event::Final(caption("世界")),
            ]
        );
    }

    #[test]
    fn plain_blank_line_clears() {
        assert_eq!(parse_plain(&[b"\n"], true), vec![Event::Clear]);
    }

    #[test]
    fn plain_eof_flushes_buffer() {
        assert_eq!(
            parse_plain(&[b"tail"], true),
            vec![Event::Final(caption("tail"))]
        );
    }

    #[test]
    fn plain_utf8_split_across_chunks() {
        let bytes = "中文".as_bytes();
        let (a, b) = bytes.split_at(2);
        assert_eq!(
            parse_plain(&[a, b, b"\n"], true),
            vec![Event::Final(caption("中文"))]
        );
    }

    #[test]
    fn plain_invalid_utf8_is_replaced() {
        let events = parse_plain(&[&[0xff, b'\n']], true);
        assert_eq!(events, vec![Event::Final(caption("\u{fffd}"))]);
    }

    #[test]
    fn plain_strips_control_characters() {
        assert_eq!(
            parse_plain(&[b"a\x1b[31mb\n"], true),
            vec![Event::Final(caption("a[31mb"))]
        );
    }

    #[test]
    fn plain_keeps_tabs() {
        assert_eq!(
            parse_plain(&[b"a\tb\n"], true),
            vec![Event::Final(caption("a\tb"))]
        );
    }

    #[test]
    fn plain_can_disable_stripping() {
        let mut parser = PlainParser::new(false);
        let mut out = Vec::new();
        parser.feed(b"a\x1bb\n", &mut out);
        parser.finish(&mut out);
        assert_eq!(out, vec![Event::Final(caption("a\x1bb"))]);
    }

    #[test]
    fn plain_truncates_overlong_line() {
        let mut parser = PlainParser::new(true);
        let mut out = Vec::new();
        let long = vec![b'x'; MAX_LINE_BYTES + 1];
        parser.feed(&long, &mut out);
        assert!(out.is_empty());
        parser.feed(b"\n", &mut out);
        match &out[0] {
            Event::Final(caption) => assert_eq!(caption.text.len(), TAIL_BYTES),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn plain_unterminated_content_is_flushed_as_partial() {
        let mut parser = PlainParser::new(true);
        let mut out = Vec::new();
        parser.feed(b"4", &mut out);
        assert!(out.is_empty(), "feed 本身不产生事件");
        parser.flush_pending(&mut out);
        assert_eq!(out, vec![Event::Partial(caption("4"))]);
    }

    #[test]
    fn plain_pending_skips_incomplete_utf8() {
        let bytes = "中".as_bytes();
        let mut parser = PlainParser::new(true);
        let mut out = Vec::new();
        parser.feed(&bytes[..2], &mut out);
        parser.flush_pending(&mut out);
        assert!(out.is_empty(), "半个字符不应显示");
        parser.feed(&bytes[2..], &mut out);
        parser.flush_pending(&mut out);
        assert_eq!(out, vec![Event::Partial(caption("中"))]);
    }

    #[test]
    fn plain_pending_blank_line_is_ignored() {
        let mut parser = PlainParser::new(true);
        let mut out = Vec::new();
        parser.feed(b"   ", &mut out);
        parser.flush_pending(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn jsonl_has_no_pending_partial() {
        let mut parser = Parser::new(InputFormat::Jsonl, true);
        let mut out = Vec::new();
        parser.feed(r#"{"event":"partial","text":"欢迎""#.as_bytes(), &mut out);
        parser.flush_pending(&mut out);
        assert!(out.is_empty());
    }

    fn parse_jsonl(input: &str) -> Vec<Event> {
        let mut parser = JsonlParser::new(true);
        let mut out = Vec::new();
        parser.feed(input.as_bytes(), &mut out);
        parser.finish(&mut out);
        out
    }

    #[test]
    fn jsonl_partial_and_final() {
        assert_eq!(
            parse_jsonl(
                "{\"event\":\"partial\",\"text\":\"欢迎\"}\n\
                 {\"event\":\"final\",\"text\":\"欢迎收听\",\"speaker\":\"A\"}\n"
            ),
            vec![
                Event::Partial(caption("欢迎")),
                Event::Final(Caption {
                    text: "欢迎收听".to_string(),
                    speaker: Some("A".to_string()),
                }),
            ]
        );
    }

    #[test]
    fn jsonl_clear_and_unknown_ignored() {
        assert_eq!(
            parse_jsonl("{\"event\":\"clear\"}\n{\"event\":\"bogus\"}\nnot json\n"),
            vec![Event::Clear]
        );
    }

    #[test]
    fn jsonl_missing_text_ignored() {
        assert!(parse_jsonl("{\"event\":\"final\"}\n").is_empty());
    }

    #[test]
    fn jsonl_strips_control_characters() {
        let events = parse_jsonl("{\"event\":\"final\",\"text\":\"a\\u001b[31mb\"}\n");
        assert_eq!(events, vec![Event::Final(caption("a[31mb"))]);
    }

    #[test]
    fn plain_truncation_is_reported_as_warning() {
        let mut parser = PlainParser::new(true);
        let mut out = Vec::new();
        let mut warnings = Vec::new();
        parser.feed(&vec![b'x'; MAX_LINE_BYTES + 1], &mut out);
        parser.drain_warnings(&mut warnings);
        assert_eq!(
            warnings,
            vec![Warning::LineTruncated {
                limit_kib: MAX_LINE_BYTES / 1024,
            }]
        );
    }

    #[test]
    fn jsonl_parse_failure_is_reported_as_warning() {
        let mut parser = JsonlParser::new(true);
        let mut out = Vec::new();
        let mut warnings = Vec::new();
        parser.feed(b"not json\n", &mut out);
        parser.drain_warnings(&mut warnings);
        assert_eq!(warnings, vec![Warning::JsonParseFailed]);
    }

    #[test]
    fn jsonl_unknown_event_is_reported_once_per_window() {
        let mut parser = JsonlParser::new(true);
        let mut out = Vec::new();
        let mut warnings = Vec::new();
        parser.feed(b"{\"event\":\"bogus\"}\n{\"event\":\"bogus\"}\n", &mut out);
        parser.drain_warnings(&mut warnings);
        // 限流窗口内只报一次。
        assert_eq!(warnings.len(), 1);
    }
}
