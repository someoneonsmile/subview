//! 用户可见的 stderr 输出。
//!
//! 所有面向用户的输出都走这里，统一 `subview: ` 前缀，便于格式一致，
//! 也为将来引入 `--quiet` 之类的开关留出单一入口。

use std::fmt::Display;

/// 普通信息或错误（统一 `subview: ` 前缀）。
pub fn note(message: impl Display) {
    eprintln!("subview: {message}");
}

/// 警告（`subview: warning: ` 前缀），用于非致命问题。
pub fn warn(message: impl Display) {
    eprintln!("subview: warning: {message}");
}

/// 错误后附示例的提示块。
pub fn note_with_examples(message: impl Display, examples: &[&str]) {
    note(message);
    eprintln!("示例：");
    for example in examples {
        eprintln!("    {example}");
    }
}
