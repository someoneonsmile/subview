//! CLI 集成测试：通过真实二进制验证协议、退出码与 I/O 行为。

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin;
use predicates::prelude::*;

fn subview() -> Command {
    Command::cargo_bin("subview").expect("构建 subview 二进制")
}

#[test]
fn default_mode_without_display_falls_back_to_terminal() {
    // 没有图形环境时默认模式会回退到终端界面；stdout 不是终端时要求 --raw。
    subview()
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .write_stdin("你好\n")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("回退"))
        .stderr(predicate::str::contains("--raw"));
}

#[test]
fn raw_prints_one_line_per_final() {
    subview()
        .args(["--raw"])
        .write_stdin("第一条\n第二条\n")
        .assert()
        .success()
        .stdout("第一条\n第二条\n");
}

#[test]
fn raw_discards_partial_results() {
    // `b\r` 是中间结果，`b2\n` 才是定稿。
    subview()
        .args(["--raw"])
        .write_stdin("a\nb\rb2\n")
        .assert()
        .success()
        .stdout("a\nb2\n");
}

#[test]
fn raw_flushes_buffer_without_trailing_newline() {
    subview()
        .args(["--raw"])
        .write_stdin("没有换行")
        .assert()
        .success()
        .stdout("没有换行\n");
}

#[test]
fn raw_ignores_clear_events() {
    subview()
        .args(["--raw"])
        .write_stdin("\n\n")
        .assert()
        .success()
        .stdout("");
}

#[test]
fn raw_jsonl_with_speaker() {
    subview()
        .args(["--raw", "--format", "jsonl"])
        .write_stdin(
            "{\"event\":\"final\",\"text\":\"你好\",\"speaker\":\"A\",\"t\":12.3}\n\
             {\"event\":\"partial\",\"text\":\"中间\"}\n\
             {\"event\":\"clear\"}\n",
        )
        .assert()
        .success()
        .stdout("[A] 你好\n");
}

#[test]
fn invalid_json_lines_are_ignored_with_warning() {
    subview()
        .args(["--raw", "--format", "jsonl"])
        .write_stdin("not json\n{\"event\":\"final\",\"text\":\"ok\"}\n")
        .assert()
        .success()
        .stdout("ok\n")
        .stderr(predicate::str::contains("warning"));
}

#[test]
fn control_characters_are_stripped_in_tui_mode() {
    if !have_script() {
        return;
    }
    let binary = cargo_bin("subview");
    let input = std::env::temp_dir().join(format!("subview-esc-{}.txt", std::process::id()));
    std::fs::write(&input, b"a\x1b[31mb\n").expect("写临时输入文件失败");

    let command = format!("{} < {}", binary.display(), input.display());
    let output = std::process::Command::new("script")
        .arg("-qec")
        .arg(command)
        .arg("/dev/null")
        .output()
        .expect("执行 script 失败");
    let _ = std::fs::remove_file(&input);

    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(text.contains("a[31mb"), "剥除后应显示为纯文本");
    assert!(!text.contains("\u{1b}[31mb"), "ESC 序列不应原样出现在终端");
}

#[test]
fn raw_mode_keeps_control_characters() {
    subview()
        .args(["--raw"])
        .write_stdin("a\x1bb\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("a\x1bb"));
}

#[test]
fn rejects_invalid_arguments() {
    subview().args(["--lines", "0"]).assert().code(2);
    subview().args(["--size", "3"]).assert().code(2);
    subview().args(["--linger", "-1"]).assert().code(2);
    subview().args(["--raw", "--overlay"]).assert().code(2);
    subview().args(["--format", "bogus"]).assert().code(2);
    subview().args(["--vertical", "--tui"]).assert().code(2);
    subview().args(["--vertical", "--raw"]).assert().code(2);
    // 旧参数已合并/改名，应被拒绝。
    subview().args(["--width", "200"]).assert().code(2);
    subview().args(["--height", "800"]).assert().code(2);
    subview().args(["--position", "bottom"]).assert().code(2);
    subview().args(["--position", "top"]).assert().code(2);
}

#[test]
fn vertical_mode_needs_x11_and_does_not_fall_back_to_terminal() {
    // 竖版只有悬浮窗支持：没有 X11 时应明确报错退出，而不是静默变成横版终端界面。
    subview()
        .args(["--vertical", "--size", "800", "--position", "end"])
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .write_stdin("你好\n")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("竖版"));
}

#[test]
fn help_and_version_work() {
    subview().arg("--help").assert().success();
    subview().arg("--version").assert().success();
}

/// 在 pty 中运行，才能覆盖“stdin 是终端”的分支。
#[test]
fn stdin_tty_prints_usage_hint() {
    if !have_script() {
        return;
    }
    let binary = cargo_bin("subview");
    let command = format!("{} --tui", binary.display());
    let output = std::process::Command::new("script")
        .arg("-qec")
        .arg(command)
        .arg("/dev/null")
        .output()
        .expect("执行 script 失败");
    let combined = [output.stdout, output.stderr].concat();
    let text = String::from_utf8_lossy(&combined);
    assert!(text.contains("标准输入"), "实际输出：{text}");
}

/// TUI 模式必须进入并离开备用屏幕。
#[test]
fn tui_enters_and_leaves_alternate_screen() {
    if !have_script() {
        return;
    }
    let binary = cargo_bin("subview");
    let command = format!("printf '一\\n' | {} --tui", binary.display());
    let output = std::process::Command::new("script")
        .arg("-qec")
        .arg(command)
        .arg("/dev/null")
        .output()
        .expect("执行 script 失败");
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(text.contains("\u{1b}[?1049h"), "缺少进入备用屏幕序列");
    assert!(text.contains("\u{1b}[?1049l"), "缺少离开备用屏幕序列");
}

/// SELFPIPE：下游关闭管道时应按 Unix 惯例收到 SIGPIPE 退出。
#[cfg(unix)]
#[test]
fn sigpipe_when_stdout_closed() {
    use std::io::Write;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command as StdCommand, Stdio};

    let binary = cargo_bin("subview");
    let mut child = StdCommand::new(binary)
        .args(["--raw"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("启动 subview 失败");

    let mut stdin = child.stdin.take().expect("stdin");
    let writer = std::thread::spawn(move || {
        let line = "字幕行 abcdefghijklmnopqrstuvwxyz\n";
        for _ in 0..200_000 {
            if stdin.write_all(line.as_bytes()).is_err() {
                break;
            }
        }
    });

    // 关闭读端，让 subview 的下一次写入触发 SIGPIPE。
    drop(child.stdout.take());
    let status = child.wait().expect("等待 subview 退出");
    assert_eq!(status.signal(), Some(libc_sigpipe()));
    writer.join().ok();
}

#[cfg(unix)]
fn libc_sigpipe() -> i32 {
    13
}

fn have_script() -> bool {
    std::process::Command::new("script")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}
