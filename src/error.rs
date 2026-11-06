//! 进程退出码与启动阶段错误。

/// 进程退出码。
///
/// 所有退出路径（正常、用法错误、运行时错误、信号终止）都收敛到这里，
/// 由 `main` 统一转换为 `i32`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// 正常结束。
    Ok,
    /// 运行时错误（环境不满足且无法回退）。
    Runtime,
    /// 用法错误（参数、I/O 方式不合法）。
    Usage,
    /// 被信号终止，退出码为 128 + 信号编号。
    Signal(i32),
}

impl ExitCode {
    pub fn as_i32(self) -> i32 {
        match self {
            Self::Ok => 0,
            Self::Runtime => 1,
            Self::Usage => 2,
            Self::Signal(signal) => 128 + signal,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_map_to_shell_conventions() {
        assert_eq!(ExitCode::Ok.as_i32(), 0);
        assert_eq!(ExitCode::Runtime.as_i32(), 1);
        assert_eq!(ExitCode::Usage.as_i32(), 2);
        assert_eq!(ExitCode::Signal(2).as_i32(), 130);
    }

    #[test]
    fn startup_errors_keep_usage_and_runtime_apart() {
        assert_eq!(
            ExitCode::Runtime,
            StartupError::VerticalNeedsOverlay.exit_code()
        );
        assert_eq!(ExitCode::Usage, StartupError::StdinIsTerminal.exit_code());
        assert_eq!(ExitCode::Usage, StartupError::TuiNeedsTty.exit_code());
    }
}
