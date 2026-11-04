# subview 设计文档

> 状态：M0 / M1 / M2 已实现并通过验收；M3（打包与发行）未做
> 决策依据：用户拍板 —— 纯显示器 / 终端为主 + 悬浮窗可选 / Linux 优先

## 1. 定位

**subview 是字幕的 pager。** 它只做一件事：把输入的字幕事件流渲染成屏幕上的实时字幕。

```
音频源 → ASR 程序(外部) → (字幕流) → subview → 屏幕
mic / 回环   whisper.cpp 等         stdin       stdout/tty
```

识别、翻译、录音、窗口管理都不在职责范围内。subview 与字幕内容来源无关：语音识别、
机器翻译、歌词、弹幕，只要能产生字幕流，就都能接。

## 2. 已确认的范围

| 决策 | 结论 |
|---|---|
| 产品边界 | 纯显示器，ASR 永远外部进程；README 提供组合配方 |
| 显示形态 | 默认桌面悬浮窗（半透明圆角字幕条 + 点击穿透）；`--tui` 终端界面、`--raw` 纯过滤器可选 |
| 平台 | X11 优先；Wayland 不在当前范围（未来可用 layer-shell 扩展）；终端模式代码保持跨平台可编译 |
| 语言 | Rust，单二进制，依赖最小化 |
| 配置 | 纯 CLI flags；悬浮窗样式（字号/颜色/阴影）暂为内置常量 |

### 非目标（明确不做）

内置 ASR、翻译、录制/转写、GUI 设置面板、主题系统、字幕历史持久化、网络功能、
插件系统。每项都会破坏「纯粹」，需要时用管道组合外部工具。

## 3. 设计原则 → 决策对照

| Unix 原则 | 对应设计 |
|---|---|
| 单一职责 | 只做渲染；不 fork、不调用 ASR |
| 文本流是接口 | 字幕事件走 stdin/stdout，可 grep、可录制、可 SSH |
| 可组合 | 任何按行输出的程序都能作 producer |
| 沉默是金 | 字幕只上屏；警告/诊断只走 stderr |
| 不重复造轮子 | 终端模式复用终端渲染；悬浮窗只做字幕层，不实现窗口管理 |
| 显式优于隐式 | 输入格式必须 `--format` 指定，不做自动探测 |
| 失败要快且大声 | 用法错误 exit 2；非法输入限流告警；终端状态必恢复 |

## 4. 输入协议

协议即 API，须稳定。两种格式，由 `--format` 显式选择。

### 4.1 `--format plain`（默认）

输入为 UTF-8 字节流。字节级语义：

| 输入 | 含义 |
|---|---|
| `\n` | **提交**：此前累积的字节作为一条 final 字幕入历史 |
| `\r` | **覆盖**：此前累积的字节作为当前 partial 显示，不提交 |
| 连续空行（立即 `\n`） | **清屏**事件：清空字幕区 |
| 其他 C0/C1 控制字符 | 显示模式剥除（`\t` 保留）；`--raw` 输出到非终端时原样透传 |
| EOF 时缓冲区非空 | 以 final 提交后退出 |

这借用终端自身的行语义：`\r` = 擦除重写，`\n` = 落定（curl / apt 进度条传统）。
好处是 `echo 你好 | subview` 可直接测试，且兼容 CRLF（`\r\n` 自然等价于先 partial 后
以相同内容 final，结果正确）。

非法 UTF-8 字节替换为 U+FFFD，绝不 panic。

### 4.2 `--format jsonl`

每行一个 JSON 对象：

```json
{"event":"partial","text":"欢迎"}
{"event":"final","text":"欢迎收听本期节目","speaker":"A","t":12.3,"lang":"zh"}
{"event":"clear"}
```

- `event` 必填：`partial` / `final` / `clear`
- `text`：`partial`/`final` 必填
- `speaker`：可选，渲染为 `[A] ` 前缀
- `t`：可选，秒级时间戳（仅供参考/调试，不影响渲染时机）
- 未知 `event`：忽略；未知字段：忽略
- 非法 JSON 行：忽略，stderr 告警（同类告警限流为每 5 秒最多一条）

### 4.3 事件语义

- partial **可丢**：渲染跟不上时只保留最新 partial，不阻塞生产者
- final **不可丢**：必须全部进入历史
- 背压仅对 partial 生效

## 5. 显示

### 5.1 终端后端（`--tui`，M0）

- 启动时进入 alternate screen，退出（含 SIGINT/SIGTERM/panic）恢复终端状态
- 字幕区底部对齐，保留最近 `--lines N` 条（默认 3）
- 未定稿行以暗色/斜体显示，定稿后转正常样式
- 按显示宽度折行（`unicode-width`，CJK/emoji/组合字符按 grapheme 边界），
  超宽部分悬挂缩进 2 列；单条字幕超出可用行数时保留尾部（实时滚动）
- 渲染节流约 20fps，合并高频 partial 更新
- SIGWINCH 后重新计算宽度并重绘

### 5.2 `--raw` 纯过滤器后端（M1）

只向 stdout 写文本与光标控制序列，不管窗口、不管历史，最纯粹：

- stdout 是 tty：ANSI 原地更新（`\r` / 清行 / 上移）
- stdout 非 tty：自动降级为纯文本（每个 final 一行），无 ANSI 字节
- 可与 `tee` 配合录制

### 5.3 桌面悬浮窗后端（默认，feature = "overlay"）

已实现并通过 X11 真机验证：

- **透明背景**：创建 32 位 ARGB visual 窗口，直接 `PutImage` premultiplied alpha
  像素；由合成器（picom/mutter/kwin/xfwm4 等）完成混合。未检测到合成器时向
  stderr 提示（此时背景显示为黑色）。
- **点击穿透**：用 SHAPE 扩展把窗口输入区域设为空
  （`shape_rectangles(SET, INPUT, [])`），鼠标事件直接落到下层窗口。
- **置顶 / 无边框 / 不抢焦点**：`override_redirect` 窗口，窗口管理器不接管，
  不占任务栏、不接受输入焦点。
- **纯 Rust**：`x11rb`（X11 协议 + shape 扩展）+ `cosmic-text`（排版与光栅化），
  不依赖 GTK/Qt，编译产物仍是单二进制。
- **显示模型**：参考 `~/yt-live-sub` 的字幕条——半透明圆角背景（黑色 alpha 30，圆角
  10px）+ 白色文字 + 柔和阴影；最近的字幕拼成连续文本流，折行后整体上移，只显示
  最后 `--lines` 行（默认 2 行），新内容从下方进入、旧行被顶上去。
- **文字可读性**：cosmic-text 输出字形覆盖率，用右下偏移的柔和阴影（多遍盒式模糊）
  改善可读性，再按白色混合文字本体；字号 24px、行高 32px、内边距 16/10px。
- 位置 `--position start|end`（横版：start 贴底、end 贴顶）；默认窗口宽度按约
  15 个全角字估算（≈410px），可用 `--size` 指定（像素，200 到屏幕宽−80），
  水平居中，底部留 60px 边距。
- 无图形环境（无法连接 X11）时返回 `Unavailable`，`main` 自动回退到终端界面。
- 与终端后端共用读线程与字幕状态；主循环轮询 X11 事件与状态更新（16ms 节流）。

已知限制（同步记录在 README）：

- 只支持 X11：Wayland 需要 layer-shell 才能实现同样的透明与点击穿透
- 透明依赖合成器；无合成器时背景为黑色
- 样式（字号 24px、行高 32px、背景 alpha 30、圆角 10px、阴影）是内置常量，尚未配置化
- 文本流只保留尾部约 400 字符，更早的内容会被丢弃

### 5.4 竖版悬浮窗（`--vertical`）

面向竖屏场景的竖排布局：字从上到下、列从右到左。

- **触发**：`--vertical`，默认横版不变；只对悬浮窗后端生效，与 `--tui` / `--raw`
  互斥。没有图形环境时直接报错退出（exit 1），不静默回退成横版终端界面。
- **尺寸**：`--size` 是唯一的尺寸参数，竖版下表示窗口高度（像素），默认
  屏幕高 − 120（最小 52px）；宽度自动 = `--lines` 列 × 字框边长 + 左右内边距
  （2 列约 96px）。横版下 `--size` 表示窗口宽度，高度自动 = `--lines` 行。
- **位置**：竖版下 `--position start|end` 沿水平轴贴边（start 贴左、end 贴右），
  垂直居中，窗口保证完整落在屏幕内。
- **排版**：cosmic-text 不支持竖排，按 grapheme 把每个字单独排进 32×32 字框
  （`Metrics(24, 32)` + 居中），再沿列方向堆叠。列从文本开头锚定：首列从顶部向下填满，
  最新的（可能未填满的）列在最左，旧列随新列出现向右滚出窗口；已有列内容不重排。
  窗口高度决定每列字数，宽度决定可见列数。
- **标点**：逗号、顿号、句号按中文竖排惯例挪到字框右上角；冒号、分号、叹号、问号
  保持居中；拉丁字母直立，不旋转。
- **共用逻辑**：背景条、阴影、linger 淡出、点击穿透、状态与主循环复用横版实现。
- **限制**：`--align` 在竖版下不生效；单个字形超过字框的部分会被裁切；
  样式常量仍不可配置。

## 6. CLI 规格

```
subview [OPTIONS]

输入
  --format plain|jsonl      输入协议，必选值（默认 plain）
显示
  --lines N                 保留历史行数（默认 3）
  --size N                  折行宽度（终端，列）/ 窗口尺寸（悬浮窗，像素：
                            横版为宽、竖版为高；另一维自动）
  --align left|center       水平对齐（默认 left）
  --linger SEC              最后一条 final 后逐行淡出并清屏的秒数（默认 0 = 常驻）
  --no-color                关闭颜色（同时遵守 NO_COLOR 环境变量）
后端
  --raw                     纯 ANSI 过滤器模式
  --tui                     强制终端界面（默认桌面悬浮窗）
  --position start|end      悬浮窗位置（横版贴底/贴顶，竖版贴左/贴右；默认 start）
  --vertical                竖版悬浮窗：字从上到下、列从右到左（仅桌面悬浮窗）
调试
  --debug                   诊断信息写 stderr
  -h, --help / -V, --version
```

互斥规则：`--raw` 与 `--tui` 互斥；`--vertical` 与 `--raw` / `--tui` 互斥。
`--lines` / `--size` / `--align` / `--linger` 在所有后端生效：`--size` 在终端模式是
折行列数、横版悬浮窗是窗口宽度、竖版悬浮窗是窗口高度；竖版下 `--align` 无效果。

## 7. Unix 契约

### 退出码

| 码 | 含义 |
|---|---|
| 0 | stdin EOF，正常结束 |
| 1 | 运行时错误（渲染器初始化失败等） |
| 2 | 用法错误（参数非法、stdin 是 tty、默认模式 stdout 非 tty） |
| 130 / 143 | 被 SIGINT / SIGTERM 中断（终端恢复后） |

### 信号

- `SIGINT` / `SIGTERM`：恢复终端状态 → 退出 130 / 143
- `SIGWINCH`：重算折行并重绘
- `SIGPIPE`：**显式恢复默认处理**，使 `subview | head` 干净退出（Rust 默认忽略会报
  broken pipe，必须处理）

### I/O 行为

- stdin 是 tty：stderr 打印用法提示（含 `producer | subview` 示例）→ exit 2
- 默认模式 stdout 非 tty：stderr 提示改用 `--raw` → exit 2（不静默改变行为）
- `--raw` 且 stdout 非 tty：纯文本降级
- 字幕只走 stdout/屏幕；所有告警、诊断走 stderr
- 非 UTF-8 输入容错；不因输入异常终止

### 环境变量

- `NO_COLOR`：存在即关闭颜色
- `TERM=dumb`：视为无 ANSI 能力，降级纯文本

## 8. 组合配方（README 部分）

```bash
# whisper.cpp 流式识别（麦克风）
arecord -q -f S16_LE -r 16000 -c 1 | \
  whisper-stream -m models/ggml-small.bin --step 0 --length 5000 2>/dev/null | \
  subview --lines 2 --align center

# 协议回放测试（无需 ASR）
printf '第一条\n第二条\r第二条（中间结果）\n' | subview --lines 1

# 纯过滤器，嵌入 tmux / 录制
producer | subview --raw --lines 1 --size 80 | tee captions.ansi
```

把常见 ASR 输出（whisper-stream / vosk-cli / 云端 CLI）转成 subview 协议的命令行
`awk`/`sed` 示例放在 README 的「Recipes」一节。若后续需要成型的适配脚本，再考虑
随仓库提供 `contrib/`，但核心仓库保持纯显示器（用户已确认边界）。

## 9. 项目结构与依赖

```
subview/
├── Cargo.toml
├── DESIGN.md
├── README.md
├── man/subview.1
├── docs/protocol.md          # 协议规范（= 本文第 4 节展开）
├── tests/
│   └── cli.rs                # CLI 集成测试（含 pty / SIGPIPE 用例）
└── src/
    ├── main.rs               # 入口：isatty、模式选择、主循环、退出码
    ├── cli.rs                # clap 定义与颜色判定
    ├── protocol.rs           # 字节流 → Event（\r/\n 切分、UTF-8 容错、控制符过滤）
    ├── reader.rs             # 读线程：解析 stdin → 共享状态 + 通知
    ├── state.rs              # 字幕窗口状态：历史 + partial + speaker
    ├── wrap.rs               # 显示宽度感知折行 + 悬挂缩进
    ├── render/
    │   ├── mod.rs            # Backend trait
    │   ├── ansi.rs           # ANSI 序列拼接原语
    │   ├── layout.rs         # 后端无关的物理行排版
    │   ├── tui.rs            # 终端后端（alternate screen）
    │   ├── raw.rs            # 原地重绘过滤器后端
    │   ├── vertical.rs       # 竖排切列（字从上到下、列从右到左）
    │   └── overlay.rs        # X11 透明悬浮窗后端（默认启用，含横版/竖版渲染）
    └── signal.rs             # SIGPIPE / SIGINT / SIGTERM
```

依赖：

- 必需：`clap`（derive）、`crossterm`、`libc`、`serde`、`serde_json`、
  `unicode-width`、`unicode-segmentation`
- overlay feature（默认启用）：`x11rb`（features = ["shape"]）、`cosmic-text`
- dev：`assert_cmd`、`predicates`
- 不引入 async runtime

线程模型：读线程解析 stdin 并更新共享状态，用 dirty 原子标志（终端后端）或
`EventLoopProxy`（悬浮窗后端）唤醒主线程；主线程以约 16ms 的节奏节流重绘。

## 10. 里程碑与验收

| 阶段 | 范围 | 验收标准 | 状态 |
|---|---|---|---|
| M0 | 协议解析 + 终端渲染 + CJK 折行 + isatty/信号/退出码 + 测试 | 管道真机可用；单元/集成测试通过；`subview \| head` 干净退出；Ctrl-C 后终端恢复正常 | 完成 |
| M1 | `--raw`、`--linger`、颜色/对齐/说话人、man page | 可 `tee` 录制；tmux 分屏可用；`man subview` 渲染正常 | 完成 |
| M2 | 桌面悬浮窗：X11 原生透明背景 + 点击穿透 + 默认启用；竖版（`--vertical`） | 透明窗口真机截图验证；指针命中下层窗口验证穿透；无图形环境回退终端；竖排离屏渲染验证 | 完成（X11 验证） |
| M3 | 打包（AUR / deb）、README recipes | 一条命令安装 | 待做 |

## 11. 实现自检清单（已全部完成）

- [x] 读线程按字节扫描而非依赖 `read_until(b'\n')`：`\r` 也是分隔符，UTF-8 可跨 chunk
- [x] 终端恢复覆盖 panic 路径：`TuiRenderer` 的 Drop guard + `catch_unwind`
- [x] SIGPIPE 显式恢复；集成测试验证 `subview --raw | head` 收到 SIGPIPE（141）
- [x] 控制字符剥除测试（含 ESC 注入用例；显示模式剥除、非 tty 过滤器透传）
- [x] CRLF 输入用例
- [x] CJK / emoji 折行不破 grapheme 用例
- [x] stdin 是 tty、stdout 非 tty 的用法错误分支用例
- [x] 退出码矩阵测试（0 / 1 / 2 / 130 / 143 / SIGPIPE）
- [x] 终端尺寸为 0×0 时退回 80×24（pty 未设置窗口大小的场景）
- [x] `linger` 淡出与清屏的真机验证（终端 + 悬浮窗）
- [x] X11 32 位 ARGB 透明窗口 + premultiplied alpha 渲染（真机截图验证）
- [x] SHAPE 输入区域为空实现点击穿透（指针命中测试验证）
- [x] 无图形环境自动回退终端界面（集成测试覆盖）
- [x] 竖排切列按文本开头锚定（已有列不重排）、grapheme 不被切开（单元测试覆盖）
- [x] 竖版几何计算（`--size` 自适应横/竖版、`--position start|end` 贴左/贴右、屏幕内 clamp）单元测试覆盖
- [x] 竖排离屏渲染验证（逐字排版、列右→左、标点右上角位移）
- 竖版尚未做 X11 真机截图验证（开发环境无图形会话）
