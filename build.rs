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
// cli_types.rs 已经引入 ValueEnum（include! 到本文件作用域），这里只能用匿名导入，避免重名导入报错。
use clap::ValueEnum as _;
use clap_complete::{Shell, generate_to};

fn main() -> Result<(), Box<dyn Error>> {
    let outdir = match env::var_os("SHELL_HELP_DIR") {
        None => return Ok(()),
        Some(outdir) => std::path::PathBuf::from(outdir),
    };

    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_owned();

    // 为所有支持的 shell 生成补全文件
    let complete_dir = outdir.join("complete");
    create_dir_all(&complete_dir)?;
    for &shell in Shell::value_variants() {
        generate_to(shell, &mut cmd, &bin_name, &complete_dir)?;
    }

    Ok(())
}
