# subview

把标准输入的字幕流实时显示到屏幕上。

**subview 是字幕的 pager：只做显示，不做识别。** 语音识别、翻译、语音合成都不在它的
职责里——那些交给上游程序，用管道接进来。

```
麦克风 / 系统回环 → ASR 程序 → (字幕流) → subview → 屏幕
                    whisper.cpp / vosk / 云端 CLI        stdin
```

默认以**桌面悬浮窗**显示：半透明圆角字幕条、白字带描边与柔和阴影、点击穿透、置顶、无边框；
已定稿字幕是纯白字，未定稿（partial）字幕用更淡的半透明白（约 43%）加细字重，
风格接近 YouTube 直播字幕（每条字幕独立成行、新内容从下方进入、旧行被顶上去）。
也可以选终端界面（`--tui`）或纯 ANSI 过滤器（`--raw`）；竖屏场景用 `--vertical`
切换成竖排（字从上到下、列从右到左）。

这也意味着它可以显示任何"字幕流"：语音识别结果、机器翻译、歌词、演讲提词、弹幕…

## 特性

- **默认桌面悬浮窗**：半透明圆角字幕条 + 白字描边与阴影，未定稿字幕半透明白显示，
  每条字幕独立成行折行显示（类似 yt-live-sub）
- **竖版悬浮窗**：`--vertical` 切换成竖排（字从上到下、列从右到左），`--size` 指定
  窗口尺寸（横版为宽度、竖版为高度），适合竖屏视频与竖屏直播
- **点击穿透**：字幕窗口不接收鼠标事件，不挡鼠标操作
- **一个二进制，一个职责**：只负责排版与显示，不做 ASR、不联网、不写日志
- **标准输入 / 标准输出**：任何能按行输出字幕的程序都能直接接
- **协议极小**：`\n` 定稿、`\r` 覆盖（借终端行语义），有需要时可选 JSONL
- **CJK 友好**：按字形边界折行，中文 / emoji / 组合字符不会被切断
- **可无图形运行**：`--tui` 终端界面、`--raw` 纯过滤器；无图形环境自动回退终端
- **无状态**：Ctrl-C 即退；不写配置、不留历史

## 环境要求

| 模式 | 要求 |
|---|---|
| 默认悬浮窗 | X11 会话（Wayland 暂不支持），以及正在运行的合成器（compositor） |
| `--vertical` | 同默认悬浮窗（竖版只有悬浮窗后端支持） |
| `--tui` | 任意支持 ANSI 的终端 |
| `--raw` | 任意终端 / 管道，无图形依赖 |

- 透明背景依赖合成器（picom / compton、mutter、kwin、xfwm4 等）。没有合成器时窗口背景
  会显示为黑色，程序会在 stderr 提示。
- 没有图形环境（SSH、headless）时，默认模式自动回退到终端界面，并在 stderr 说明；
  使用 `--vertical` 时则直接报错退出（不会回退成横版终端界面）。
- 纯终端版可以直接 `--no-default-features` 编译，完全不引入 X11 相关依赖。

## 安装

### 预编译二进制

从 [GitHub Releases](https://github.com/someoneonsmile/subview/releases) 下载最新稳定版：

| 平台 | 资产 |
|---|---|
| Linux x86_64 (glibc) | `subview-x86_64-linux-gnu.tar.gz` |
| Linux x86_64 (musl，静态) | `subview-x86_64-linux-musl.tar.gz` |
| Linux ARM64 | `subview-aarch64-linux-gnu.tar.gz` |

每个压缩包内含二进制、bash/zsh/fish 补全、man 手册页与 README/LICENSE。解包后把
`subview` 放进 `PATH`，补全与 man 页按包内路径安装即可。

### AUR（Arch Linux）

```bash
# 稳定版
yay -S subview-bin
# 或
paru -S subview-bin

# 每夜构建（最新 commit）
yay -S subview-nightly-bin
```

### 从源码构建

```bash
git clone https://github.com/someoneonsmile/subview && cd subview
just release      # 生成二进制 + shell 补全
just install      # 安装到 /usr/local（二进制 + 补全 + man）
```

不需要 just 时，直接用 cargo：

```bash
cargo build --release
install -m755 target/release/subview ~/.local/bin/
# 或
cargo install --path .
```

纯终端版（不编译悬浮窗，无 X11 依赖）：

```bash
cargo build --release --no-default-features
cargo install --path . --no-default-features
```

## 快速开始

不需要 ASR 也能先玩。默认会弹出桌面悬浮窗，字幕显示 5 秒后自动关闭：

```bash
sh -c 'printf "你好，世界\n这是 subview\n第二条字幕\n"; sleep 5' | subview
```

中间结果（`\r` 覆盖当前行）会让字幕"边识别边刷新"：

```bash
sh -c 'printf "正在识别\r这是实时字幕\n"; sleep 5' | subview
```

终端界面（适合 SSH 或不想弹窗时）：

```bash
printf '你好，世界\n这是终端字幕\n' | subview --tui
```

竖版悬浮窗（字从上到下、列从右到左，适合竖屏画面）：

```bash
sh -c 'printf "竖排字幕\n第二条字幕\n第三条字幕\n"; sleep 5' | \
  subview --vertical --size 600
```

与语音识别组合：

```bash
# 以 whisper.cpp 的 whisper-stream 为例（参数按你的模型调整）
arecord -q -f S16_LE -r 16000 -c 1 | \
  whisper-stream -m models/ggml-small.bin --step 0 --length 5000 2>/dev/null | \
  subview --lines 2 --align center
```

## 输入协议

默认 `--format plain`，借用终端自己的行语义：

| 输入 | 含义 |
|---|---|
| `\n` | 提交一条字幕（定稿） |
| `\r` | 用当前内容覆盖显示（中间结果，可被丢弃） |
| 尚未结束的内容 | 实时作为中间结果显示（无需 `\r`，可被后续内容更新） |
| 空行 | 清屏 |
| `\r\n`（CRLF） | 一次行结束 |

需要时间戳、说话人等信息时用 `--format jsonl`：

```json
{"event": "final", "text": "欢迎收听本期节目", "speaker": "A"}
{"event": "partial", "text": "欢迎"}
{"event": "clear"}
```

完整规范（含字节级语义、背压约定、给 producer 作者的示例）见
[docs/protocol.md](docs/protocol.md)。

## 与 ASR 组合

任何按行输出文本的程序都能直接接 `subview`。以下是几个适配模式：

```bash
# 1) 逐行输出的 ASR：每行就是一条定稿字幕
my-asr --stream | subview

# 2) 只想显示"当前这一句"：把每行变成覆盖式 partial
my-asr --stream | awk '{printf "%s\r", $0; fflush()}' | subview --lines 1

# 3) 自带时间戳的 whisper 输出：去掉时间戳后显示
whisper-stream ... 2>/dev/null | sed -E 's/^\[[^]]*\] *//' | subview

# 4) 边显示边录制
producer | subview --raw | tee session.log
```

> `subview` 不替你做 ASR，也不内置任何识别模型——这是刻意的：识别工具在不断进化，
> 显示器只需要把接口做稳定。

## 命令行

```
subview [OPTIONS]

  --format <plain|jsonl>   输入格式（默认 plain）
  --lines <N>              显示的字幕行数（默认 2，最小 1；终端模式取 min(终端高度, N)）
  --size <N>               折行宽度（终端，列）/ 窗口尺寸（悬浮窗，像素）：
                           横版是窗口宽度、竖版是窗口高度，另一维自动
  --align <left|center>    水平对齐（默认 left）
  --linger <SEC>           最后一条字幕驻留多少秒后淡出并清空（默认 5，0 = 常驻）
  --no-color               关闭终端的颜色与文字属性（悬浮窗的定稿/未定稿配色不受影响）
  --background-alpha <N>   悬浮窗背景条不透明度（0–255，0 = 完全透明，默认 30）
  --raw                    纯 ANSI 过滤器模式：不接管屏幕，原地更新
  --tui                    强制使用终端界面（默认是桌面悬浮窗）
  --position <start|end>  悬浮窗位置：横版 start 贴底 / end 贴顶，
                           竖版 start 贴右 / end 贴左（默认 start）
  --vertical               竖版悬浮窗：字从上到下、列从右到左（仅桌面悬浮窗）
  --debug                  把诊断信息打到 stderr
  -h, --help / -V, --version
```

- 默认是悬浮窗；没有图形环境时自动回退到终端界面
- `--vertical` 只对默认悬浮窗生效，与 `--tui` / `--raw` 互斥；没有图形环境时直接报错退出，
  不会静默回退成横版终端界面
- `--size` 是唯一的尺寸参数：终端模式是折行列数；悬浮窗横版是窗口宽度、竖版是窗口高度，
  另一维自动算（横版高度 = `--lines` 行，竖版宽度 = `--lines` 列）
- `--position start|end` 的方位随布局：横版沿垂直轴（start 贴底、end 贴顶），
  竖版沿水平轴（start 贴右、end 贴左，垂直居中）
- `--align` 在竖版下不生效
- `--background-alpha` 只对悬浮窗生效，透明白底需要合成器，否则会显示为黑色
- `--tui` 需要 **stdout 是终端**；输出到管道或文件时请用 `--raw`
- `--raw` 且 stdout 非终端时自动降级为纯文本输出（每个定稿一行），可安全重定向
- `--raw` 与 `--tui` 互斥

## 显示形态

| 模式 | 适用场景 |
|---|---|
| 默认（悬浮窗） | 桌面字幕条：透明背景、点击穿透、置顶、无边框，覆盖在视频/会议画面上 |
| `--vertical` | 竖屏场景：竖排字幕条（字从上到下、列从右到左），覆盖在竖屏视频/直播画面上 |
| `--tui` | 开一个专用终端窗口放字幕；可 SSH、tmux、录制 |
| `--raw` | 嵌进 tmux 分屏、在已有终端里原地更新、或管道录制 |

## 环境变量

- `NO_COLOR`：存在即关闭颜色（同 `--no-color`，只影响终端后端）
- `TERM=dumb`：终端模式下视为无 ANSI 能力，降级纯文本

## 退出码

| 码 | 含义 |
|---|---|
| 0 | stdin 结束，正常退出 |
| 1 | 运行时错误 |
| 2 | 用法错误（参数非法，或输出目标不是终端且未加 `--raw`） |
| 130 / 143 | 被 SIGINT / SIGTERM 中断 |

`subview | head` 这类下游提前关闭的场景按 Unix 惯例安静退出（SIGPIPE）。

## 开发

日常命令由 [just](https://github.com/casey/just) 管理（`just --list` 查看全部）：

```bash
just check                          # 快速类型检查
just build                          # debug 构建（同时生成 shell 补全到 shell_help/）
just release                        # release 构建
just test                           # 测试（默认特性，部分需要 util-linux 的 script）
just test-terminal                  # 测试（纯终端特性）
just fmt-check && just lint-strict  # 格式检查 + 严格 clippy
just ci                             # fmt → clippy → test → release 全量检查
```

不用 just 时等价的 cargo 命令：

```bash
cargo build --release                          # 默认：含悬浮窗
cargo build --release --no-default-features    # 纯终端版
cargo test                                     # 单元 + 集成测试（部分需要 util-linux 的 script）
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --no-default-features -- -D warnings
cargo fmt --check
```

shell 补全由 `build.rs` 在设置 `SHELL_HELP_DIR` 时生成到 `shell_help/complete/`
（普通 `cargo build` 不会生成）；`man/subview.1` 是手写维护的权威版本。

## 设计文档

- [DESIGN.md](DESIGN.md)：架构、范围、Unix 契约与里程碑
- [docs/protocol.md](docs/protocol.md)：输入协议规范

## 已知限制

- 悬浮窗只支持 X11；Wayland 尚未支持（后续可用 layer-shell 实现同样的透明与穿透）
- 透明背景依赖合成器；没有合成器时背景为黑色
- 点击穿透只在 X11 下生效
- 悬浮窗样式（字号 24px、行高 32px、描边、阴影、圆角 10px、定稿纯白/未定稿淡白+细字重配色）
  是内置常量，尚未支持配置；背景不透明度可用 `--background-alpha` 调整
- 悬浮窗每条字幕独立成行，显示最后 `--lines` 行，更早的内容自动滚出视野
- 悬浮窗横版下 `--size` 的范围是 200 像素到「屏幕宽度 − 80」；竖版下是 52 像素到
  「屏幕高度」；终端模式下是折行列数
- 竖排是逐字排版（cosmic-text 不支持竖排）：标点只对逗号/顿号/句号做了竖排位置调整，
  拉丁字母保持直立，单个字形超过 32×32 字框的部分会被裁切
- `--raw` 的原地重绘依赖光标控制；若终端里同时有大量其他输出，可能出现错位

## 许可证

MIT，见 [LICENSE](LICENSE)。
