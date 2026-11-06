# 代码结构评审与重构方案

> 评审基线：`34b9ae2`（工作区干净）。
> 范围：`src/` 全部模块、`build.rs`、`tests/cli.rs`；不含 CI / 打包 / README 文案。
> 方法：全量阅读源码 + 与 `DESIGN.md` 对照 + `cargo clippy --all-targets`（当前零告警）。
>
> 结论先行：功能实现是扎实的——依赖严格单向、无循环、clippy 干净、关键算法有测试。
> 主要问题不在功能，而在“继续加功能会变贵”的结构债，集中在四件事：
> **重复的运行流程**、**过大的 overlay 模块**、**CLI 类型穿透到渲染层**、**协议层带 I/O 副作用**。
> **状态：已确认并进入实施。** 决策：按 5 个阶段全做；overlay 完整接入统一主循环；`Backend` 统一为单一 `Renderer`（内容级接口，非像素级）。实施进度记录见文末。

## 0. 现状速览

| 模块 | 行数 | 职责 | 主要问题 |
|---|---|---|---|
| `main.rs` | 316 | 入口、模式分发、终端主循环、纯文本过滤器 | 职责过载（S2） |
| `render/overlay.rs` | 1168 | X11 窗口、事件泵、帧循环、横/竖版排版、像素合成 | 上帝模块（S1） |
| `protocol.rs` | 584 | 字节流 → Event | 直接写 stderr（R1） |
| `render/vertical.rs` | 215 | 竖排切列 | 位置放错（S3） |
| `render/layout.rs` | 166 | 终端物理行排版 | 被 main 反向引用（C2） |
| `render/tui.rs` / `raw.rs` | 160 / 128 | 终端后端 | draw 前置计算重复（S4） |
| `state.rs` | 113 | 历史 + partial | 状态消费方式三套（R5） |
| `reader.rs` | 72 | 读线程 | 与 main 共享裸 bool（B2） |
| `cli.rs` / `cli_types.rs` | 123 / 98 | clap 定义与转换 | 类型穿透到渲染层（B1） |
| `wrap.rs` / `signal.rs` / `render/ansi.rs` | 126 / 43 / 75 | 折行 / 信号 / ANSI 原语 | 无 |

体量分布：`overlay.rs` 占 `src/` 的 34%，`main.rs` + `overlay.rs` 合计 43%。

## 1. 问题地图

| 编号 | 维度 | 问题 | 优先级 | 相对工作量 |
|---|---|---|---|---|
| A1 | 架构 | 主循环与淡出状态机两份实现 | P0 | M |
| A2 | 架构 | 模式矩阵分散，无单一事实来源 | P0 | M |
| A3 | 架构 | DESIGN.md 线程模型描述失实 | P1 | S |
| A4 | 架构 | overlay 渲染与 X11 强绑定，CI 无法回归 | P0 | M |
| A5 | 架构 | `Backend` 只覆盖终端后端，统一为 `Renderer` | P0 | M |
| S1 | 结构 | `overlay.rs` 是上帝模块（1168 行） | P0 | L |
| S2 | 结构 | `main.rs` 职责过载 | P1 | M |
| S3 | 结构 | `render/` 概念混层，vertical 放错位置 | P2 | S |
| S4 | 结构 | 终端尺寸/布局参数重复计算 | P1 | S |
| R1 | 职责 | protocol 层带 I/O 副作用 | P0 | S |
| R2 | 职责 | 错误类型与退出码不统一 | P1 | M |
| R3 | 职责 | 用户输出散落 25 处 | P2 | S |
| R4 | 职责 | `--raw` 一分为二，命名不达意 | P1 | S |
| R5 | 职责 | 状态消费方式三套，语义各自解读 | P1 | M |
| B1 | 抽象 | CLI 类型穿透到渲染层 | P0 | M |
| B2 | 抽象 | `strip_control` 裸 bool 多层传递 | P1 | S |
| B3 | 抽象 | `fade: u8` 裸值穿三层接口 | P2 | S |
| B4 | 抽象 | overlay 的“整串文本 + 字节偏移”契约脆弱 | P0 | M |
| B5 | 抽象 | `--size` 一个参数三种语义 | P2 | S |
| C1 | 耦合 | main 与 overlay 的“半个 app”耦合 | P0 | L |
| C2 | 耦合 | `render::layout` 被 main 反向引用 | P1 | S |
| C3 | 耦合 | 双特性组合的 cfg 分散 | P1 | S |
| C4 | 耦合 | 主循环/linger 无直接测试 | P1 | M |
| C5 | 耦合 | `Caption` 协议 DTO 与显示模型合一 | P2 | 不做 |

优先级定义：P0 = 重构主干，强烈建议做；P1 = 配套，建议做；P2 = 顺带或记录，可不做。
相对工作量：S < 0.5 天，M = 0.5–1.5 天，L > 1.5 天（单人）。

---

## 2. 架构层

### A1. 主循环与淡出状态机两份实现（P0）

**现状**
- `src/main.rs:190` 的 `drive()` 实现“dirty 轮询 → linger 淡出 → EOF 退出”。
- `src/render/overlay.rs:297` 的 `event_loop()` 再实现一遍同样的状态机（`last_event` / `fade` / `linger_done` 三变量、相同公式），额外插入 X11 事件泵。
- `FADE_SECS` 与 `TICK` 在 `src/main.rs:31,33` 和 `src/render/overlay.rs:69,71` 各定义一次。

**影响**
- linger 语义（触发阈值、淡出曲线、清屏时机）改一处漏一处，两份常量已经处于“可能漂移”状态。
- overlay 侧的状态机无法单测（需要 X11），终端侧可以。
- 每新增一个后端就会复制第三份。

**方案**
新增 `src/run/mod.rs`，抽出两件东西：

```rust
pub struct Linger { /* last_event, fade, done */ }
impl Linger {
    pub fn on_event(&mut self);
    /// 返回 Some(新 fade) 表示需要重绘；None 表示无变化。
    pub fn tick(&mut self, now: Instant, linger: f64) -> Option<u8>;
    pub fn is_done(&self) -> bool;
}

pub fn drive<R: Renderer>(renderer: &mut R, state: &Arc<Mutex<CaptionState>>,
                          dirty: &AtomicBool, eof: &AtomicBool, linger: f64) -> Result<(), AppError>;
```

`Renderer` 是统一抽象（见 A5）：`poll()` 供 overlay 泵 X11 事件、报告 Expose，终端后端默认返回 `Idle`；`draw(state, fade)` 画一帧。
overlay 的 `event_loop` 收缩为：`WindowContext` 实现 `Renderer`，然后调用 `run::drive`。
删除 overlay 内重复的常量，统一从 `run` 导出。

---

### A2. 模式矩阵分散，无单一事实来源（P0）

**现状**
- `src/main.rs:43` 的 `run()` 用 if/else 交织：stdin tty 检查、`--raw` 的 tty/非 tty 分叉、`--tui`、overlay、feature cfg、vertical 特例、回退文案。
- `src/main.rs:89` / `:104` 两段 `#[cfg]` 平行代码，各自带一份错误文案。
- 行为矩阵（后端 × stdout tty × feature）只存在于实现与 README，没有可测的单一事实来源。

**影响**
- 改回退策略或新增后端要动多段 if；组合容易漏（vertical + no-overlay 的文案目前就分在两处）。
- 集成测试只能覆盖部分组合。

**方案**
新增 `src/app.rs`：

```rust
pub enum Mode { Overlay, Tui, RawTty, PlainFilter }
pub struct Capabilities { pub overlay: bool }

pub fn resolve(cli: &Cli, stdin_tty: bool, stdout_tty: bool, caps: Capabilities)
    -> Result<Mode, AppError>;
```

`resolve` 是纯函数，把全部组合写成一条决策链（顺序：stdin tty → `--raw` → `--tui` → `--vertical`/overlay 能力 → 回退）。
`main` 只做 `match resolve(...)` 分发；错误文案集中在 `AppError` 的 `Display`。
为 `resolve` 写表驱动单测（4 后端 × 2 stdout tty × 2 能力），集成测试保留关键行为验证。

---

### A3. DESIGN.md 线程模型描述失实（P1）

**现状**：`DESIGN.md:292-293` 称悬浮窗后端用 `EventLoopProxy` 唤醒主线程；实际 `overlay.rs` 用的是 `AtomicBool` + 16ms 轮询，项目中不存在 `EventLoopProxy`。

**影响**：设计文档是架构的权威描述，失实会误导后续重构者。

**方案**：随 A1 落地后更新为“读线程 + dirty 原子标志 + 固定节流轮询”，并补一张“模式 × I/O”矩阵（可直接引用 `app::resolve` 的决策链）。

---

### A4. overlay 渲染与 X11 强绑定，CI 无法回归（P0）

**现状**
- `overlay.rs` 36 个函数中只有 `compute_geometry` 有测试（`src/render/overlay.rs:962` 起）。
- 像素合成（背景 / 阴影 / 描边 / 遮罩 / 混合 / 打包）与横竖两套排版全部无测试。
- CI 无 X11，只能验证“无 DISPLAY 回退”这一条路径。

**影响**：改渲染参数（阴影、描边、竖排标点）没有任何自动化保护，只能肉眼验证。

**方案**：把纯计算抽到不依赖 X11 的模块（见 S1 的 `canvas.rs` / `compose.rs`），补单测：
- `blend_pixel` 的 premultiplied 语义（透明底、完全覆盖、alpha=0）；
- `pixels_to_bytes` 的 LSB/MSB 字节序；
- `outline_mask` / `blur_mask` 的边界（radius=0、1px 画布、全 0 遮罩）；
- `compose_stream` 的截断与 partial 边界（现有 5 个测试迁移后保留并补 B4 的用例）。

---

### A5. `Backend` 抽象只覆盖终端后端（P0，已确认统一）

**现状**：`src/render/mod.rs:17` 的 `Backend` 只被 `tui.rs` / `raw.rs` 实现；`drive` 只服务它们；overlay 不走此 trait；纯文本过滤器完全不进渲染层。

**影响**：名字暗示“所有后端”，实际是“终端后端”；三类输出各自为政，行为一致性靠人肉。

**方案（已确认：统一为单一 `Renderer`）**：把 trait 提升为内容级接口并改名：

```rust
pub trait Renderer {
    /// 拉取后端特有事件（overlay 泵 X11；终端后端默认 Idle）。
    fn poll(&mut self) -> Result<Poll, RenderError> { Ok(Poll::Idle) }
    /// 绘制一帧。输出介质尺寸、颜色策略由实现自己持有。
    fn draw(&mut self, state: &CaptionState, fade: Fade) -> Result<(), RenderError>;
}
```

关键设计点：
- **统一发生在内容层，不是像素层**。trait 吃 `CaptionState` + `Fade`，各后端自己决定物理行（终端）与像素（overlay），不引入 `Frame` 像素抽象。
- **尺寸从 trait 签名中移除**：终端后端在 draw 内部取 `crossterm::terminal::size()`（含 0×0 兜底），overlay 持有窗口尺寸。`drive` 因此不再依赖 crossterm，成为纯流程控制。
- `Poll::Exposed` 让 overlay 的 Expose 事件触发重绘，终端后端恒返回 `Idle`。
- 纯文本过滤器没有“帧”的概念，不实现 `Renderer`，单独放在 `run/filter.rs`。

---

## 3. 结构层

### S1. `overlay.rs` 是上帝模块（P0）

**现状**：单文件 1168 行、36 个函数、占 `src/` 的 34%。承担：X11 连接与窗口生命周期、ARGB visual 选择、compositor 检测、几何计算、事件泵、帧循环、linger 状态机、cosmic-text 整形、横版绘制、竖版绘制、背景条、阴影、描边、掩码运算、像素混合、字节序打包、调试输出。

**影响**
- review 与定位成本高；一个改动（例如背景透明度）同时牵动窗口代码与像素代码。
- 纯算法与 I/O 混在一起，直接造成 A4 的测试缺口。

**方案**：拆为 `src/render/overlay/` 目录：

| 文件 | 内容 | 目标行数 |
|---|---|---|
| `mod.rs` | `OverlayBackend`：open / draw / close 生命周期、`Outcome` | ~150 |
| `window.rs` | `WindowContext`：X11 连接、窗口、visual、compositor 警告、几何 | ~250 |
| `canvas.rs` | `Frame` 像素层：背景、遮罩、模糊、混合、打包 | ~200 |
| `compose.rs` | `compose_stream`：state → 结构化内容（见 B4） | ~100 |
| `horizontal.rs` | 横版：cosmic-text 整段排版 + 光栅合成 | ~150 |
| `vertical.rs` | 竖排切列 + 单字框绘制（从 `render/vertical.rs` 迁入） | ~250 |

原则：先机械搬运 + `cargo test`，保持零行为变更；再改内部接口。每个文件 ≤ 300 行。

---

### S2. `main.rs` 职责过载（P1）

**现状**：316 行、10 个函数，承担入口、panic 兜底、isatty、模式分发、终端主循环、纯文本过滤器、终端尺寸、状态锁封装。

**影响**：入口文件越长，“改入口”的语义越模糊；`run_plain_text`（43 行）与 reader 线程的解析逻辑平行重复。

**方案**
- `main.rs` 只留 `main()` + `run() -> ExitCode`（约 40 行）。
- `app.rs`：`resolve` + 分发。
- `run/terminal.rs`：`stream_loop` + `drive` + 两个终端后端装配。
- `run/filter.rs`：`PlainFilter`（由 `run_plain_text` 迁移，见 R4）。

---

### S3. `render/` 概念混层，vertical 放错位置（P2）

**现状**：`render/` 同时放终端后端（tui / raw / ansi / layout）与图形后端（overlay / vertical）；`vertical.rs` 只被 overlay 使用，却与 tui / raw 平级。

**影响**：按目录找代码容易误判依赖关系；`vertical` 的名字让人以为终端也支持竖排。

**方案**：`vertical.rs` 迁入 `render/overlay/vertical.rs`。终端原语（`ansi.rs`、`layout.rs`）保留在 `render/` 根，不再细分 `terminal/` 子目录（收益不抵成本）。

---

### S4. 终端尺寸/布局参数重复计算（P1）

**现状**：`tui.rs:96-109` 与 `raw.rs:82-94` 的 draw 开头几乎是相同的 6 行（`cols.max(1)`、`width = limit.unwrap_or(cols).min(cols).max(1)`、`max_lines`、`layout(...)`）；终端尺寸获取在 `main.rs:254`。

**影响**：改布局参数推导要改两处。

**方案**：从 `render/layout.rs` 导出

```rust
pub fn effective_options(cols: u16, rows: u16, width_limit: Option<u16>,
                         max_lines: usize, align: Align, fade: u8) -> (LayoutOptions, usize);
```

两个终端后端的 `draw` 共用；`max_lines` 的语义（`--lines` 与终端高度取 min）只在一处实现。

---

## 4. 职责层

### R1. protocol 层带 I/O 副作用（P0）

**现状**：`src/protocol.rs:132-141`、`:289-302` 直接 `eprintln!` 超长行警告；`:344-354` 的 `warn()` 带 `Instant` 限流状态。

**影响**
- 解析器不再是纯函数；单测无法断言警告。
- 警告格式与 UI 策略耦合在协议层，“警告去哪、是否限流”无法由调用方决定。

**方案**（推荐 A）
- A：解析输出结构化警告。`feed` / `finish` / `flush_pending` 除事件外追加 `Vec<Warning>`（或统一 `enum Output { Event, Warn(Warning) }`），由 `reader` / `main` 的 `WarningSink` 负责打印与限流。`Warning` 只有四类：长行截断、JSON 解析失败、未知事件、缺 text。
- B（更小）：注入 `warn: &mut dyn FnMut(&str)` 回调，限流仍留在解析器。

推荐 A：可测试、职责干净，且限流逻辑可复用（overlay 的 compositor 警告也能走同一个 sink）。

---

### R2. 错误类型与退出码不统一（P1）

**现状**：`main` 用 `i32` + `io::Result`；`overlay` 用 `Result<(), String>` + `Outcome`；`protocol` 混 `eprintln`；退出码 0 / 1 / 2 / 128+n 分别产生在 `main.rs`、`signal.rs:40`、`overlay.rs`。

**影响**：错误路径无法统一断言；新增错误来源到处拼字符串；退出码语义只靠约定。

**方案**
- `src/exit.rs`：`enum ExitCode { Ok = 0, Usage = 2, Runtime = 1 }`，`impl From<ExitCode> for i32`；`signal::exit_code()` 统一返回 `ExitCode`。
- `src/error.rs`（可与 exit.rs 合并）：`enum AppError { Usage(String), Runtime(String), Unavailable(String) }`，`Display` 统一 `subview: ...` 前缀；`Unavailable` 专用于“overlay 不可用、可回退”。
- run 系列返回 `Result<ExitCode, AppError>`；`main` 只打印一次并转换。

---

### R3. 用户输出散落（P2）

**现状**：25 处 `eprintln!`，分布在 `protocol.rs` / `main.rs` / `overlay.rs`；`subview:` 前缀手写 20 次。

**影响**：格式一致性风险；无法统一加 `--quiet` 之类的开关。

**方案**：R1 / R2 落地后，剩余输出收敛到 `ui.rs` 的 `warn()` / `hint()` / `error()`（内部统一前缀）。
不为 25 处调用造抽象层——R1 / R2 之后如果只剩个位数，就保持直接 `eprintln!`。

---

### R4. `--raw` 一分为二，命名不达意（P1）

**现状**：`render/raw.rs` 是“tty 上的原地重绘后端”；stdout 非 tty 时 `src/main.rs:263` 走“纯文本过滤器”（只输出 Final）。两者共用 `--raw` 旗标，但一个在 render 层、一个在 main 层。

**影响**：`--raw` 的语义要“看环境”；新读者容易以为 `raw.rs` 就是 `--raw` 的全部实现；纯过滤器没有独立单测。

**方案**（用户 API 不变，只改内部命名与位置）
- `render/raw.rs`：类型改名 `RawTtyRenderer`（明确“仅 tty”）。
- `run_plain_text` 迁到 `run/filter.rs`，类型化 `PlainFilter { format, policy }`，方法 `run(stdin, stdout) -> ExitCode`。
- 行为矩阵进文档（附录），不再靠记忆。

---

### R5. 状态消费方式三套（P1）

**现状**
- `tui` / `raw`：`layout::layout` 迭代 `history()+partial()`，按宽度折行、按行数裁剪。
- `overlay`：`compose_stream` 把字幕重拼成单串文本，再用字节偏移标记未定稿。
- 纯过滤器：只取 `Final`。

**影响**：speaker 前缀、字幕间换行、400 字截断、partial 着色四件事在 overlay 里各有一份实现；状态层无法约束“渲染层能看到什么”。

**方案**：`state.rs` 增加只读视图：

```rust
pub struct CaptionView<'a> { pub captions: Vec<(&'a Caption, bool)> } // bool = partial
impl CaptionState { pub fn view(&self, max_chars: usize) -> CaptionView<'_>; }
```

`layout` 与 overlay 都从 `view()` 取数据；`compose_stream` 只负责“把 view 拼成渲染器需要的格式”。
纯过滤器保持只取 `Final`（不同关注点，不强行统一）。

---

## 5. 抽象层

### B1. CLI 类型穿透到渲染层（P0，最值得修的一处）

**现状**：`overlay::run(cli, ...)`（`src/render/overlay.rs:82`）、`WindowContext::new(cli)`（`:187`）、`compute_geometry(cli, ...)`（`:118`）、`event_loop(cli, ...)`（`:297`）全部直接读 `Cli` 字段；渲染层单测必须构造 `Cli`（`Cli::try_parse_from`）。

**影响**
- CLI 改字段就牵动 overlay 内部多处。
- overlay 无法脱离 clap 测试（几何测试被迫过 clap）。
- 渲染层依赖的是“参数怎么解析”，而不是“渲染需要什么”。

**方案**

```rust
// src/config.rs
pub struct OverlayConfig {
    pub lines: u16, pub size: Option<u16>, pub align: Align,
    pub position: Position, pub vertical: bool,
    pub background_alpha: u8, pub linger: f64, pub debug: bool,
}
pub struct TerminalConfig { pub lines: u16, pub size: Option<u16>,
    pub align: Align, pub color: bool, pub linger: f64 }
pub struct TextPolicy { pub strip_control: bool }
```

- `cli.rs` 实现 `From<&Cli> for OverlayConfig` / `TerminalConfig` / `TextPolicy`，以及 `PositionArg -> Position` 的领域化转换。
- `overlay::run(&OverlayConfig, TextPolicy)`；`compute_geometry(&OverlayConfig, ...)` 的测试直接构造配置，不再过 clap。
- `render` 层不再 `use crate::cli::*`。

---

### B2. `strip_control` 裸 bool 多层传递（P1）

**现状**：`main` → `reader::spawn_reader(..., format, strip_control, ...)`、`overlay::run(..., format, strip_control)`、`run_plain_text(format, false)` 都是裸 bool；语义“是否剥控制符”与“是否 tty”相关但不等价。

**影响**：两个简单类型相邻传参，顺序错位不会报错；语义只在 `protocol.rs` 注释里。

**方案**：并入 B1 的 `TextPolicy`；`reader::spawn_reader` 收 `ReaderOptions { format, policy }`。消除 bool 参数。

---

### B3. `fade: u8` 裸值穿三层接口（P2）

**现状**：`TerminalBackend::draw(..., fade: u8)`、`RenderOptions.fade`、`LayoutOptions.fade`，0–100 的语义靠注释；与 `background_alpha: u8`（0–255）同在 `RenderOptions` 里，数值域相邻易混。

**方案**：`struct Fade(u8)`（构造时 clamp）或字段改名 `fade_percent`。顺带做即可。

---

### B4. overlay 的“整串文本 + 字节偏移”契约脆弱（P0）

**现状**
- `compose_stream`（`src/render/overlay.rs:836`）把字幕拼成一条字符串，按 400 字截断后再 `trim_start`，`partial_start` 用 `saturating_sub` 修正。
- `vertical::split_columns_tagged`（`src/render/vertical.rs:24`）再按字节偏移给 grapheme 打标记。

**影响**
- 截断点落在 partial 中间时，`partial_start` 会被归零，整串被当作未定稿上色（潜在显示 bug；现有测试只覆盖正常路径，见 `overlay.rs:1134`）。
- 契约是“字节偏移”，但所有使用方都按字符/行思考，容易出错。

**方案**：`compose_stream` 改为返回结构化内容：

```rust
pub struct ComposedLine { pub text: String, pub partial: bool }
pub struct Composed { pub lines: Vec<ComposedLine> } // 截断后
```

- 横版：逐行拼成带换行的整串时，用行边界决定着色分段，不再用字节偏移。
- 竖版：直接吃行的 grapheme 序列；`split_columns_tagged` 入参改为 `&[ComposedLine]`，去掉 `partial_start`。
- 截断规则改为“按行截断到 ≤400 字”，保证行完整，彻底去掉字节偏移。

**实施（已完成）**：`compose` 返回 `Composed { lines: Vec<ComposedLine> }`，每行带 `partial` 标记；
截断按行进行（先整行丢弃最旧内容，单行超长时保留尾部）；竖排改用
`split_columns_lines(&[ComposedLine])`，不再接收 `partial_start`；横版 `as_text()`
由行边界推导整串与未定稿起点。

---

### B5. `--size` 一个参数三种语义（P2）

**现状**：终端 = 折行宽度（列）；横版 overlay = 窗口宽度（px）；竖版 overlay = 窗口高度（px）。注释在 `src/cli_types.rs:55-61`，分支在 `src/render/overlay.rs:118` 与终端后端的 `width_limit`。

**影响**：对用户是合理设计（一个参数自适应），对代码读者是持续的认知负担。

**方案**：不改用户 API；在 `OverlayConfig` / `TerminalConfig` 里用 `width_px` / `height_px` / `width_cols` 区分承接，文档补一张“参数 × 后端”语义表。

---

## 6. 耦合层

### C1. main 与 overlay 的“半个 app”耦合（P0）

**现状**：`overlay::run` 内部自己 spawn reader、自己持有主循环、自己调 `signal::exit_code()`，用 `Outcome` 把“可回退”策略告诉 `main`；`main` 再实现回退与 vertical 特例（`src/main.rs:89-113`）。

**影响**：overlay 是“独立 app”，入口只剩 if；回退策略分散在结果类型与入口代码两处；vertical 特例嵌在回退分支中。

**方案**（与 A1 / A2 / B1 合并解决）
- `overlay::open(&OverlayConfig) -> Result<OverlayBackend, Unavailable>`：只做资源初始化。
- `OverlayBackend` 实现统一 `Renderer`（A5），交给 `run::drive`；退出码由 `ExitCode` 统一返回。
- “vertical 在无 overlay 能力时不可回退”由 `app::resolve` 在模式解析阶段拒绝（`AppError::Usage`），不再藏在 main 的回退代码块里。

---

### C2. `render::layout` 被 main 反向引用（P1）

**现状**：`src/main.rs:311` 的纯文本输出调用 `crate::render::layout::speaker_prefix`。

**影响**：层次倒置（入口的文本输出依赖渲染排版模块）；`speaker_prefix` 本质是 caption 的文本格式化规则。

**方案**：把 `speaker_prefix` 移到 `state.rs`（caption 的显示格式化）或新 `text.rs`；`layout`、`filter`、`overlay::push_caption` 统一引用。推荐放 `state.rs`，避免新增模块。

---

### C3. 双特性组合的 cfg 分散（P1）

**现状**：`#[cfg(feature = "overlay")]` 出现在 `src/main.rs:89,104`、`src/render/mod.rs:5,9`、`src/cli.rs:7`（`cfg_attr`）；vertical 不可用文案在 main 的两个分支里。

**影响**：新增 feature 门控模块时要同步多处；`--no-default-features` 的行为差异靠人记。

**方案**：随 A2 落地，`Capabilities { overlay: bool }` 由 `cfg!` 在 main 一处构造，其余逻辑与 cfg 解耦；`render/mod.rs` 的模块声明 cfg 保留（不可避免）。

---

### C4. 主循环/linger 无直接测试（P1）

**现状**：单元测试集中在纯算法；集成测试通过真实二进制 + `script` 模拟 pty；主循环、linger 淡出、EOF 退出、dirty 节流没有直接测试。

**影响**：A1 的重复逻辑没有回归保护；重构主循环只能靠集成测试间接覆盖。

**方案**：A1 抽出 `Linger` 与 `drive` 后——`Linger` 注入 `Instant` 参数（不依赖真实时钟），测触发阈值、淡出进度、完成清屏；`drive` 用假 `Sink` + 假 dirty/eof，测“无变化不重绘”“EOF 补最后一帧”“信号中断路径”。

---

### C5. `Caption` 协议 DTO 与显示模型合一（P2，不做）

**现状**：`protocol::Caption` 被 state / layout / overlay / main 直接使用。

**影响**：当前合理（只有一种输入）；若未来支持 SRT / VTT 等多种输入，会被输入协议绑死。

**方案**：不改，记录为已知边界。未来需要时再引入 `DisplayCaption` 转换层。

---

## 7. 目标结构

```
src/
├── main.rs              # 约 40 行：main() + 退出码
├── app.rs               # 模式解析 resolve + 分发
├── cli.rs               # clap 定义 + Cli → 配置转换
├── cli_types.rs         # 不变（build.rs 双编译约束）
├── config.rs            # OverlayConfig / TerminalConfig / TextPolicy / ReaderOptions
├── exit.rs              # ExitCode
├── error.rs             # AppError（可与 exit.rs 合并）
├── protocol.rs          # 纯解析：bytes → Event + Warning（无 I/O）
├── state.rs             # CaptionState + CaptionView + speaker_prefix
├── reader.rs            # 读线程 + WarningSink
├── signal.rs            # 不变
├── wrap.rs              # 不变
├── run/
│   ├── mod.rs           # Linger + drive + Sink
│   ├── terminal.rs      # stream_loop + tui/raw 装配
│   └── filter.rs        # PlainFilter（非 tty 纯文本输出）
└── render/
    ├── mod.rs           # Renderer trait（统一内容级接口）
    ├── layout.rs        # 终端排版
    ├── ansi.rs
    ├── tui.rs
    ├── raw.rs           # RawTtyRenderer
    └── overlay/
        ├── mod.rs       # OverlayBackend：open / draw
        ├── window.rs    # X11 资源与几何
        ├── canvas.rs    # 像素 / 背景 / 阴影 / 描边 / 混合 / 打包
        ├── compose.rs   # state → Composed（结构化）
        ├── horizontal.rs
        └── vertical.rs  # 竖排切列 + 绘制（原 render/vertical.rs）
```

文件数从 16 增至约 22，换来的是“入口薄、流程集中、绘制分文件、配置独立”。

## 8. 分阶段计划

| 阶段 | 内容 | 提交 | 验收 |
|---|---|---|---|
| 0 准备 | `config.rs`；B1/B2 配置化；C2 `speaker_prefix` 迁移；R1 警告回调 | 1 | `just ci` 绿，行为不变 |
| 1 主循环 | A1 + A5 + C1 + C4 + R2：`Linger` / `drive` / `Renderer`，overlay 接入；`exit.rs` / `error.rs`；删重复常量；补单测 | 1–2 | ci 绿；手动对比 overlay 淡出/退出 |
| 2 拆分 overlay | S1 + A4 + S3：机械拆模块；canvas 纯函数补单测；vertical 迁入 | 1–2 | ci 绿；横版/竖版真机截图对比 |
| 3 入口与模式 | A2 + S2 + R4 + C3：`app.rs` + `run/terminal.rs` + `run/filter.rs`；resolve 表驱动测试 | 1 | ci 绿；`tests/cli.rs` 全过 |
| 4 收尾 | A5 重命名、S4 `effective_options`、B3/B5 命名、文档更新 | 1 | `just ci` + `just test-terminal` 绿 |

风险与回滚：每阶段独立提交，可单独回退。阶段 1 若风险偏高，可先只抽 `Linger`（不动 `drive` 结构）消掉 overlay 的重复状态机，再决定是否统一循环。
行为变更仅限循环节流实现（仍为 16ms），可用真机对比验证。

## 9. 明确不做

- 不引入 async runtime（项目明确非目标）。
- 不引入像素级 `Frame` 抽象：统一后的 `Renderer` 是内容级接口（吃 `CaptionState` + `Fade`），终端后端仍走 ANSI 物理行，overlay 仍走像素合成。
- 不把 crate 改成 lib + bin（无外部复用需求，只增加维护面）。
- 不改 `--size` 的用户语义（只改内部命名）。
- 不动 `build.rs` 的 `include!` 机制（clap 补全生成的最小方案）。
- 不为 25 处 `eprintln!` 造统一 UI 层（R1/R2 之后剩余量可能已很小）。

## 10. 已确认决策

1. **范围**：按 5 个阶段全做（P0 + P1）。
2. **主循环**：overlay 完整接入 `run::drive`。
3. **Backend 抽象**：统一为单一 `Renderer`（内容级接口，不引入像素级 Frame）。
4. **节奏**：逐阶段推进，每阶段一个可独立回退的提交。
5. **新增文件**：接受文件数从 16 增至约 22。

## 11. 验收标准

- `just ci` 全绿（fmt-check、两种 feature 的 clippy、两种 feature 的 test、release 构建）。
- `tests/cli.rs` 全部通过；除明确记录的行为修正外，不修改既有断言。
- 附录行为矩阵逐项验证不变。
- 无新增 `#[allow]`；clippy 零告警。
- 文档同步：`DESIGN.md` 结构章节与线程模型、`AGENTS.md` 目录结构、`README.md` 受影响说明。
- 提交信息符合 `type: 中文描述`；每阶段一个可独立回退的提交。

## 附录：行为矩阵（重构回归基线）

| 场景 | stdin 是 tty | stdout 是 tty | DISPLAY 可用 | 行为 | 退出码 |
|---|---|---|---|---|---|
| 默认 | 否 | 任意 | 是 | overlay 横版 / 竖版（`--vertical`） | 0，信号 128+n |
| 默认 | 否 | 是 | 否 | 回退终端界面（备用屏幕） | 0 |
| 默认 | 否 | 否 | 否 | 提示回退失败，要求 `--raw` | 2 |
| 默认 + `--vertical` | 否 | 任意 | 否 | 报错：竖版仅悬浮窗支持 | 1 |
| `--tui` | 否 | 是 | 任意 | 备用屏幕渲染 | 0 |
| `--tui` | 否 | 否 | 任意 | 报错：需要终端输出 | 2 |
| `--raw` | 否 | 是 | 任意 | 原地重绘字幕区 | 0 |
| `--raw` | 否 | 否 | 任意 | 纯文本过滤，只输出 Final 行 | 0 |
| 任意 | 是 | 任意 | 任意 | 用法提示：从管道读取 | 2 |

## 12. 实施记录

实际实施顺序为阶段 0 → 1 → 3 → 2 → 4（先做入口与模式，再拆 overlay），每阶段独立验证。

| 阶段 | 状态 | 实际改动 |
|---|---|---|
| 0 准备 | 完成 | 新增 `config.rs`（`OverlayConfig` / `TextPolicy` / `ReaderOptions`）；`Cli` → 配置转换；`speaker_prefix` 下沉到 `state`；`protocol` 改为产出 `Warning`，由 `reader` / `filter` 负责输出 |
| 1 主循环 | 完成 | 新增 `run/mod.rs`（`Input` / `Linger` / `drive`）；`Backend` 统一为 `Renderer`（`Fade` / `Poll`）；`tui` / `raw` / `overlay` 接入；删除重复的 `FADE_SECS` / `TICK`；补 `Linger` 与 `drive` 单测 |
| 3 入口与模式 | 完成 | 新增 `app.rs`（`Mode` / `resolve` 表驱动测试）、`error.rs`（`ExitCode` / `StartupError`）、`run/terminal.rs`、`run/filter.rs`；`main.rs` 从 316 行降到 134 行 |
| 2 拆分 overlay | 完成 | `overlay.rs`（1101 行）拆为 `overlay/{mod,window,canvas,compose,horizontal,vertical}.rs`；`render/vertical.rs` 并入 `overlay/vertical.rs`；画布纯函数集中到 `canvas.rs` |
| 4 收尾 | 完成 | 更新 `DESIGN.md` 第 9 节与 `AGENTS.md` 目录结构；`--size` 语义保留不变并在本文档记录 |
| 5 补充 | 完成 | **B4 结构化 compose**：`compose` 改为结构化行 + 行级 partial 标记，竖排切列不再依赖字节偏移；新增 compose / vertical 测试 8 个（默认特性单元测试 89 → 98） |

验收结果：`just ci` 全绿（fmt-check、两种 feature 的严格 clippy、两种 feature 的测试、
release 构建）；`tests/cli.rs` 15 个集成测试未修改断言全部通过。

未落地项（有意保留）：

- **R2 的完整 `AppError`**：当前用 `error.rs` 的 `ExitCode` + `StartupError` 覆盖启动路径，
  运行时错误仍是 `io::Error` / 字符串。收益不足以支撑全链路改造，暂不引入统一错误枚举。
- **R3 统一 UI 层**：`protocol` 的两处警告已收敛到 `reader` / `filter`，其余 `eprintln!`
  集中在错误路径，不再抽 `ui.rs`。
- **B5 内部重命名**：`--size` 的用户语义不变；`OverlayConfig::size` 的注释已说明
  “横版=窗口宽、竖版=窗口高”，不在代码里拆成两个字段（一个 CLI 参数映射两义，
  拆字段反而增加转换复杂度）。
