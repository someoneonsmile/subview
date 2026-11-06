# AGENTS.md — subview

把标准输入的字幕流实时显示到屏幕：桌面悬浮窗（默认）、终端界面（`--tui`）或纯 ANSI
过滤器（`--raw`）。单二进制 Rust CLI，不做 ASR、不联网、无状态。

## 常用命令（优先用 `just`）

```bash
just check              # cargo check（快速类型检查，不生成二进制）
just build              # debug 构建（SHELL_HELP_DIR=shell_help，同时生成 shell 补全）
just release            # release 构建（同上）
just run -- --help      # 运行 debug 版并传参
just test               # cargo test（默认特性；部分用例需要 util-linux 的 script）
just test-terminal      # cargo test --no-default-features（纯终端特性）
just fmt                # cargo fmt
just fmt-check          # cargo fmt --check
just lint               # cargo clippy --all-targets
just lint-strict        # cargo clippy --all-targets -- -D warnings
just lint-strict-terminal  # 纯终端特性下的严格 clippy
just fix                # cargo clippy --fix --allow-dirty --allow-staged
just clean              # cargo clean + 删除 shell_help/
just ci                 # 全量门禁：fmt-check → clippy×2 → test×2 → release 构建
just install            # 安装到 /usr/local（二进制 + bash/zsh/fish 补全 + man）
```

## 工具链

- **Rust edition 2024** → 最低 Rust **1.85**；CI 使用 `dtolnay/rust-toolchain@stable`
  （含 `rustfmt`、`clippy`），未固定 MSRV
- 无自定义 `rustfmt.toml` / `clippy.toml`，全部使用工具默认配置
- 两种特性组合都必须过：默认 `overlay`（X11 悬浮窗）与 `--no-default-features`（纯终端）

## CI / Release

| Workflow | 触发 | 内容 |
|---|---|---|
| `ci.yml` | push / PR 到 `main` | `fmt-check`、严格 clippy（两种特性）、`cargo test`（两种特性） |
| `release.yml` | tag `v*` | 调用 `build.yml` 构建全部 Linux target，发布 GitHub Release，自动推送 AUR `subview-bin` |
| `nightly.yml` | 每日 cron / 手动 | 同 release，但发布滚动 `nightly` prerelease，自动推送 AUR `subview-nightly-bin` |

### 发布构建矩阵（仅 Linux）

| Target | Runner | Asset |
|---|---|---|
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | `subview-x86_64-linux-gnu.tar.gz` |
| `x86_64-unknown-linux-musl` | `ubuntu-latest`（`cross`） | `subview-x86_64-linux-musl.tar.gz` |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | `subview-aarch64-linux-gnu.tar.gz` |

每个 asset 解包后是 `<asset>/` 目录，内含 `subview` 二进制、`complete/`（shell 补全）、
`man/subview.1`、`README.md`、`LICENSE`。

## shell 补全与 man

- `build.rs` 通过 `include!("src/cli_types.rs")` 复用 CLI 定义；仅当设置了 `SHELL_HELP_DIR`
  时把 bash/zsh/fish/elvish/powershell 补全生成到 `$SHELL_HELP_DIR/complete/`，
  普通 `cargo build` 静默跳过。`shell_help/` 是生成产物，已加入 `.gitignore`
- `src/cli_types.rs` 被 build script 直接编译，**禁止添加 `use crate::...`**；
  需要 crate 内类型时放到 `src/cli.rs`
- 不自动生成 man：`man/subview.1` 是手写维护的权威版本（内容比 clap 自动生成的更完整），
  安装与打包都使用它

## AUR

项目维护两个 AUR 包（源码在 `aur/`、`aur-nightly/`）：

| 包名 | 目录 | AUR 远程 | 发布触发 |
|---|---|---|---|
| `subview-bin`（稳定版） | `aur/` | `subview-bin.git` | `release.yml`（tag `v*`） |
| `subview-nightly-bin`（每夜构建） | `aur-nightly/` | `subview-nightly-bin.git` | `nightly.yml`（每日 cron） |

两者都是「下载 GitHub Release 预编译 tar.gz」的 `-bin` 包，通过
「clone AUR 仓库 → 复制 `PKGBUILD`/`.SRCINFO` → commit → push」维护（AUR 禁止 force push，
clone 保证 push 可 fast-forward），共享同一个 AUR 账户的 SSH key。

```bash
# 稳定版
just aur-srcinfo            # 重新生成 aur/.SRCINFO
just aur-release VERSION    # 更新版本号 + 推送

# 每夜构建
just aur-nightly-srcinfo    # 重新生成 aur-nightly/.SRCINFO
just aur-nightly-release    # 按 当前日期+提交数+短 SHA 计算 pkgver + 推送
```

### 自动发布

`release.yml` / `nightly.yml` 的 `aur` job 会在发布后自动：更新 `PKGBUILD` 的
`pkgver`（稳定版用 tag 版本号；nightly 用 `CARGO_VER+nightly+DATE+COUNT+gSHA`）并重置
`pkgrel=1` → 用 Arch Linux Docker 容器运行 `makepkg --printsrcinfo` 生成 `.SRCINFO` →
clone AUR 仓库、复制文件、commit、push（带重试）。

**前置条件**：GitHub 仓库需配置 secret `AUR_SSH_PRIVATE_KEY`（对应已上传到 AUR 账户的公钥）。

## 目录结构

```
src/
├── main.rs       入口：参数解析、退出码
├── app.rs        运行模式解析（单一事实来源）+ 启动错误输出
├── cli.rs        CLI 的 crate 内入口（re-export + From 转换 + CLI 测试）
├── cli_types.rs  build.rs 可 include 的 CLI 定义（禁止 use crate::）
├── config.rs     渲染层 / 读线程的领域配置
├── error.rs      启动错误与错误退出码
├── protocol.rs   输入协议解析（plain / jsonl，无 I/O 副作用）
├── reader.rs     stdin 读取与背压，负责输出解析警告
├── signal.rs     SIGINT / SIGTERM 处理与终端恢复
├── state.rs      字幕状态（滚动窗口、partial 覆盖、speaker 前缀）
├── wrap.rs       CJK / emoji 友好的折行
├── run/
│   ├── mod.rs        通用主循环 drive + Linger 淡出状态机
│   ├── terminal.rs   终端后端装配（--tui / --raw）
│   └── filter.rs     非终端纯文本过滤器
└── render/
    ├── mod.rs      Renderer trait、Fade、Poll、终端尺寸
    ├── layout.rs   排版、尺寸与对齐
    ├── tui.rs      终端界面（备用屏幕）
    ├── ansi.rs     ANSI 工具
    ├── raw.rs      原地重绘（--raw 且输出到终端）
    └── overlay/    X11 悬浮窗（feature overlay）
        ├── mod.rs       生命周期、帧循环、横竖版调度
        ├── window.rs    X11 资源与几何
        ├── canvas.rs    像素层：背景 / 遮罩 / 混合 / 打包
        ├── compose.rs   状态 → 连续文本流
        ├── horizontal.rs 横版绘制
        └── vertical.rs  竖排切列与绘制
tests/cli.rs      通过真实二进制的集成测试（协议、退出码、I/O 行为）
man/subview.1     手写 man 手册页
docs/protocol.md  输入协议规范
docs/refactor-plan.md  结构评审与重构记录
DESIGN.md         架构、范围、Unix 契约与里程碑
```

## 约定

- Commit messages：`type: 中文描述`（`feat` / `fix` / `refactor` / `docs` / `chore` / `test` / `perf`）
- `justfile` 是开发命令的权威来源，优先 `just <cmd>` 而不是裸 cargo
- 最小变更：只改与任务直接相关的文件，不夹带探索痕迹
- 改命令行参数 / 协议 / 显示行为时，同步更新 `README.md`、`man/subview.1`、
  `DESIGN.md` 与 `docs/protocol.md` 中受影响的部分
