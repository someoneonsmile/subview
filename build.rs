// 构建脚本：生成 shell 补全文件。
// 只有在设置了 SHELL_HELP_DIR 环境变量时才生成到 $SHELL_HELP_DIR/complete/；
// 未设置时静默跳过（普通 cargo build 不生成这些文件）。
// 这里只生成 shell 补全，不生成 man 手册页：仓库内手写的 man/subview.1 才是权威版本，
// 内容比 clap 自动生成的更完整。

include!("src/cli_types.rs");

use std::env;
use std::error::Error;
use std::fs::create_dir_all;

use clap::CommandFactory;
use clap_complete::{Shell, generate_to};

fn main() -> Result<(), Box<dyn Error>> {
    // 补全内容由 clap 定义决定；这两个文件变了才需要重新生成。
    println!("cargo:rerun-if-changed=src/cli_types.rs");
    println!("cargo:rerun-if-changed=build.rs");
    // 环境变量本身也要触发重跑，否则先 cargo build 再 SHELL_HELP_DIR=... cargo build
    // 会命中缓存、不生成补全。
    println!("cargo:rerun-if-env-changed=SHELL_HELP_DIR");

    // 空值视为未设置：否则 `SHELL_HELP_DIR= cargo build` 会把相对路径解析到源码树根。
    let outdir = match env::var_os("SHELL_HELP_DIR").filter(|value| !value.is_empty()) {
        None => return Ok(()),
        Some(outdir) => std::path::PathBuf::from(outdir),
    };

    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_owned();

    // 项目只支持 Unix，补全只生成 Unix 下常用的 shell，不生成 PowerShell。
    let shells = [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::Elvish];
    let complete_dir = outdir.join("complete");
    create_dir_all(&complete_dir)?;
    for shell in shells {
        generate_to(shell, &mut cmd, &bin_name, &complete_dir)?;
    }

    Ok(())
}
