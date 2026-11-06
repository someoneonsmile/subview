//! 启动阶段的错误与进程退出码。

/// 错误退出码。
///
/// 正常退出与信号退出由 [`crate::signal::exit_code`] 决定（信号为 128+n），
/// 这里只表达两种失败：运行时错误与用法错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// 运行时错误（环境不满足且无法回退）。
    Runtime,
    /// 用法错误（参数、I/O 方式不合法）。
    Usage,
}

impl ExitCode {
    pub fn as_i32(self) -> i32 {
        match self {
            Self::Runtime => 1,
            Self::Usage => 2,
        }
    }
}

/// 启动阶段（模式解析）的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupError {
    /// 标准输入是终端，没有字幕流可读。
    StdinIsTerminal,
    /// 终端界面需要终端输出。
    TuiNeedsTty,
    /// 竖版只有悬浮窗后端支持，而该二进制没有编译悬浮窗能力。
    VerticalNeedsOverlay,
}

impl StartupError {
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::VerticalNeedsOverlay => ExitCode::Runtime,
            Self::StdinIsTerminal | Self::TuiNeedsTty => ExitCode::Usage,
        }
    }
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StdinIsTerminal => f.write_str("从标准输入读取字幕流，而不是从终端"),
            Self::TuiNeedsTty => f.write_str("终端界面需要终端输出"),
            Self::VerticalNeedsOverlay => f.write_str("竖版界面需要桌面悬浮窗"),
        }
    }
}
