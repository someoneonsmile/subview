# subview

把标准输入的字幕流实时显示到屏幕上。

**subview 是字幕的 pager：只做显示，不做识别。** 语音识别、翻译、语音合成都不在它的
职责里——那些交给上游程序，用管道接进来。

```
麦克风 / 系统回环 → ASR 程序 → (字幕流) → subview → 屏幕
                    whisper.cpp / vosk / 云端 CLI        stdin
```

默认以**桌面悬浮窗**显示：半透明圆角字幕条、白字带柔和阴影、点击穿透、置顶、无边框，
风格接近 YouTube 直播字幕（连续文本流、新内容从下方进入、旧行被顶上去）。
也可以选终端界面（`--tui`）或纯 ANSI 过滤器（`--raw`）。

这也意味着它可以显示任何"字幕流"：语音识别结果、机器翻译、歌词、演讲提词、弹幕…

## 特性

- **默认桌面悬浮窗**：半透明圆角字幕条 + 白字阴影，连续文本流折行（类似 yt-live-sub）
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
| `--tui` | 任意支持 ANSI 的终端 |
| `--raw` | 任意终端 / 管道，无图形依赖 |

- 透明背景依赖合成器（picom / compton、mutter、kwin、xfwm4 等）。没有合成器时窗口背景
  会显示为黑色，程序会在 stderr 提示。
- 没有图形环境（SSH、headless）时，默认模式自动回退到终端界面，并在 stderr 说明。
- 纯终端版可以直接 `--no-default-features` 编译，完全不引入 X11 相关依赖。

## 安装

```bash
git clone <仓库地址> subview && cd subview
cargo build --release
install -m755 target/release/subview ~/.local/bin/
```

或者：

```bash
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
  --lines <N>              显示最近几条字幕 / 悬浮窗中显示几行（默认 2，最小 1）
  --width <N>              终端模式：折行列数；悬浮窗模式：窗口宽度（像素）
  --align <left|center>    水平对齐（默认 left）
  --linger <SEC>           最后一条字幕驻留多少秒后淡出（默认 0 = 常驻）
  --no-color               关闭颜色（悬浮窗中未定稿字幕与已定稿同色）
  --raw                    纯 ANSI 过滤器模式：不接管屏幕，原地更新
  --tui                    强制使用终端界面（默认是桌面悬浮窗）
  --position <bottom|top>  悬浮窗位置（默认 bottom）
  --debug                  把诊断信息打到 stderr
  -h, --help / -V, --version
```

- 默认是悬浮窗；没有图形环境时自动回退到终端界面
- `--tui` 需要 **stdout 是终端**；输出到管道或文件时请用 `--raw`
- `--raw` 且 stdout 非终端时自动降级为纯文本输出（每个定稿一行），可安全重定向
- `--raw` 与 `--tui` 互斥

## 显示形态

| 模式 | 适用场景 |
|---|---|
| 默认（悬浮窗） | 桌面字幕条：透明背景、点击穿透、置顶、无边框，覆盖在视频/会议画面上 |
| `--tui` | 开一个专用终端窗口放字幕；可 SSH、tmux、录制 |
| `--raw` | 嵌进 tmux 分屏、在已有终端里原地更新、或管道录制 |

## 环境变量

- `NO_COLOR`：存在即关闭颜色（同 `--no-color`）
- `TERM=dumb`：终端模式下视为无 ANSI 能力，降级纯文本

## 退出码

| 码 | 含义 |
|---|---|
| 0 | stdin 结束，正常退出 |
| 1 | 运行时错误 |
| 2 | 用法错误（参数非法，或输出目标不是终端且未加 `--raw`） |
| 130 / 143 | 被 SIGINT / SIGTERM 中断 |

`subview | head` 这类下游提前关闭的场景按 Unix 惯例安静退出（SIGPIPE）。

## 构建与测试

```bash
cargo build --release                          # 默认：含悬浮窗
cargo build --release --no-default-features    # 纯终端版
cargo test                                     # 单元 + 集成测试（部分需要 util-linux 的 script）
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --no-default-features -- -D warnings
cargo fmt --check
```

## 设计文档

- [DESIGN.md](DESIGN.md)：架构、范围、Unix 契约与里程碑
- [docs/protocol.md](docs/protocol.md)：输入协议规范

## 已知限制

- 悬浮窗只支持 X11；Wayland 尚未支持（后续可用 layer-shell 实现同样的透明与穿透）
- 透明背景依赖合成器；没有合成器时背景为黑色
- 点击穿透只在 X11 下生效
- 悬浮窗样式（字号 24px、行高 32px、背景不透明度 30、阴影、圆角 10px）是内置常量，尚未支持配置
- 悬浮窗显示连续文本流的最后 `--lines` 行，更早的内容自动滚出视野
- 悬浮窗 `--width` 的范围是 200 像素到「屏幕宽度 − 80」；终端模式下是折行列数
- `--raw` 的原地重绘依赖光标控制；若终端里同时有大量其他输出，可能出现错位
