# Agent 弹窗 UI 打磨交付说明（2026-11-20）

> 目标：把「Agent 完成通知 / 审批 / 提问 / 本轮回复」的弹窗从半成品打磨到可交付状态；除阻塞性 bug 外不动其他功能。
> 计划：`docs/superpowers/plans/2026-11-20-agent-popup-polish-plan.md`
> 参考：`docs/superpowers/research/2026-11-20-island-hud-card-ui-spec.md`（island-hud 的布局/字号/动效数值）
> 现状勘察：`docs/superpowers/research/2026-11-20-daozai-agent-ui-current.md`
> E2E 验收：`docs/superpowers/research/2026-11-20-agent-ui-acceptance.md`

---

## 1. 交付内容

| 区域 | 之前 | 现在 |
|---|---|---|
| 画布 | 960×270，详情最高 262 | 960×**520**，详情最高 500；命中/窗口区域仍只覆盖岛体+卡片，桌面其余区域照常点击穿透 |
| 审批卡 | 360 宽、字符数估算高度、正文不滚动会截断、胶囊按钮无主次 | **419 宽（=面板宽）**、真实排版测量、工具/操作内容/申请理由分区、长命令滚动不截断、按钮等分（主操作反白）、禁用态可读、入口带「收起」 |
| 提问卡 | 一题一屏 + 简单选项 | 选项带描述与完整选中态、多选计数、流内自定义输入、最后一题「检查答案」→ 审阅页「确认提交」、禁用原因可见、作答前后按钮不跳位、Ctrl+Enter |
| 完成通知 | 轻提示/展开卡两套硬编码坐标；「查看」是死按钮 | 两形态统一布局、完成徽标、摘要去 markdown 标记；**「查看回复」打开可滚动结果层**（受限 markdown：标题/列表/引用/代码块，复制内容，关闭即收起） |
| 状态 | 只有「能否允许」 | 等待（呼吸描边 + 队列 `1 / N` + 交回倒计时）、提交中、提交失败「结果未知」（保留卡片 + 错误行）、队列计数 |
| 动效 | 高度弹簧，其余硬切 | 卡片入场淡入+上移（200ms，延迟 60ms）、按钮 hover 140ms / 按下 3% 缩放、滚动条、等宽数字（倒计时/队列不跳宽）、阻塞卡片不再随鼠标离开自动收起 |
| 字体 | 只有比例字体 | 新增等宽家族（Cascadia Mono → Consolas），用于命令行、代码块、倒计时、数字 |

**交互语义未变**：关闭详情 ≠ 交回；「回客户端」才交回；不改写宿主授权范围；请求正文不落盘、不进历史；题量/选项/题干上限与去重规则保持；同一 id 重发只保留一条。

---

## 2. 代码结构

```
src/agent_ui/           新增，唯一表现层入口
├── mod.rs              视图结构体（ApprovalView/QuestionView/DoneView/ResultView）、State/Action、入场曲线
├── metrics.rs          尺寸/字号 token + 真实排版测量器 M + 按钮/chip/滚动条/勾选等原语
├── widgets.rs          头部（含跳转/收起）、状态行、底部按钮行、滚动视口、错误行
├── banner.rs           收起态待处理提示卡
├── approval.rs         审批详情
├── question.rs         提问详情（多题/多选/审阅页/输入框）
└── done.rs             完成通知两形态 + 本轮回复结果层（受限 markdown）
src/agent_cards.rs     只留状态机：队列、倒计时、交回、协议发送、结果层开关；detail_size/draw_details 为薄壳
src/app.rs             画布高度、Agent 面板尺寸/收起策略、结果层接线
src/extras.rs          draw_notice 的 Agent 分支委托 agent_ui
src/theme.rs           Agent 语义色 + 等宽字体家族
```

**关键不变量**

1. `measure_*` 与 `draw_*` 必须用同一套布局（`agent_ui::tests` 里有 6 条断言守着：宽度不撑开、高度 ≤ 画布、作答前后高度一致、入场单调）。
2. 不要覆盖内置 `FontFamily::Monospace`：设置页分段控件的文字垂直居中依赖它的默认度量（覆盖会让 `settings_theme_follows_system_and_preserves_island_style` 差 1.25px 失败）。等宽字体只注册 `mono-regular/semibold/bold` 三个命名家族。
3. 阻塞式卡片（审批/提问/结果层）不参与「鼠标离开自动收起」，只由收起/决定/交回/点击卡片外关闭。
4. 结果层关闭后必须同步 `open=false`，否则留下空岛（`result_was_active` 逻辑）。

---

## 3. 验证

| 项 | 结果 |
|---|---|
| `cargo test --lib`（全量，单线程） | 见本轮记录：Agent 相关全绿；非 Agent 仅剩本机环境限制项 |
| Agent 专项（`cargo test --lib agent`） | 37 通过 / 0 失败（含新增 7 条 agent_ui 布局单测 + 结果层 UI 用例） |
| 画面导出 | `DAOZAI_AGENT_PREVIEW=<目录> cargo test --lib agent_cards_gallery -- --ignored` 一次导出 11 个场景（审批/长审批/原生授权/队列/单选/多选/多轮/多选项/轻提示/展开卡/结果层） |
| 提问卡专项导出 | `DAOZAI_QUESTION_PREVIEW_DIR=<目录> cargo test --lib agent_ui::question::tests -- --ignored` 导出 7 个场景（含审阅页未答/已答、异常态） |
| 真机探针 | `node scripts/agent-card-probe.mjs <DAOZAI_DATA_DIR> approval|question|finished|queue` → 真实窗口截图 + 鼠标点击，桥接返回决定 |
| 真实 DSH E2E | `scripts/agent-e2e.ps1 -Scenario all`（真实 DSH 插件链 → 岛仔 → OCR 定位 → 真点击 → 校验决定）：**exit 0 / ok=true**，审批 `allowed-once`、提问 `selected=["冒烟测试"]`、完成 `done`；证据 `docs/superpowers/research/agent-e2e-shots/20261120-051519/` |

### 已知限制

- 无 UIA 文本：屏幕侧验收依赖 OCR；小字号（10px 以下）按钮在 100% 缩放下识别不稳定，E2E 用多倍率放大 + 关键词多候选规避。
- 结果层的 markdown 是**受限解析**（只识别行首 `#`/`##`/`###`/`-`/`>`/```），表格、图片、复杂嵌套不渲染；解析不到就整段当普通段落。
- 滚动只处理滚轮，未做拖拽滚动条与惯性滚动。
- 视图不能选中文字（egui 自绘），复制走结果层的「复制内容」。

### 排查开关

真机上看不出卡片状态时（比如「卡片该出现却没出现」），用 **debug 构建**加环境变量 `DAOZAI_AGENT_TRACE=1` 启动，每 6 帧把 notice/open/details/队列/active/hover 追加写入
`%TEMP%\daozai-agent-trace.log`（release 也带这段代码，但只在设置该变量时才写）。排查完记得把变量去掉。

---

## 4. 交付

- exe：`target\x86_64-pc-windows-msvc\release\daozai.exe`
- 大小 **22,748,672** 字节｜SHA256 `4F6D68D62EA865CB9BC7B9FC4CB1C14A20406B048B69DB0C3FCB20B7B61D72EB`
- 桌面快捷方式「岛仔－当前测试版」已指向该文件（已核对目标/大小/哈希一致）
- 验收证据：`docs/superpowers/research/agent-e2e-shots/20261120-051519/`（20 个文件，截图已裁到卡片本体）

## 5. 本机开发环境（重要）

本机 VS 18 Community 缺少桌面 CRT（`lib\x64`、`include`），直接用 vcvars 会报 `LNK1104 msvcrt.lib` / `C1083 vcruntime.h`。统一用：

```powershell
powershell -NoProfile -Command ". .\scripts\dev-msvc-env.ps1; cargo build --locked --release --bin daozai --target x86_64-pc-windows-msvc"
```

脚本会调用 **BuildTools** 的 `vcvars64.bat`，并把 `CC/CXX/AR` 钉死到该实例（否则 cc-rs 会挑到 Community，`zstd-sys`/`ring` 编译失败）。

快速迭代用 debug 构建（约 10–20s）：`cargo build --bin daozai --target x86_64-pc-windows-msvc`；release（LTO+codegen-units=1）约 5 分钟。
