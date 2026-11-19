# daozai 复刻 island-hud 总体设计方案

日期：2026-11-19
依据：`docs/superpowers/research/daozai-gap-audit.md`、`hud-transport-contract.md`、`hud-session-jump.md`、`hud-renderer-semantics.md`
目标：在 **Rust + eframe/egui** 的 daozai 上复刻 island-hud 的 Agent 交互能力，视觉与动效沿用 daozai 现有体系。

---

## 1. 能力范围决策

### 1.1 复刻（本期做）

| 能力 | 现状 | 目标 | 价值 |
| --- | --- | --- | --- |
| 未知来源/类型请求回交 | 静默丢弃（已修） | 一律 `decision=return` | 防客户端卡死 |
| 契约校验 | 仅题量 | 对齐 island-hud：选项 ≤50、题干 ≤20000、答案结构校验、单选互斥 | 防脏数据拖垮 UI |
| 两阶段提交 | 发出即移除 | 提交后保留至终态，失败可恢复 | 避免「点了但没生效」 |
| 结果未知态 | 无 | 提交失败锁控件、保留草稿 | 防误判已生效 |
| 过期态 | 无 | `receivedAt+timeout` 判定 + 文案 | 与宿主一致性 |
| 会话跳转 | 无 | Codex 深链 + 前台兜底 | 点卡回原会话 |
| 正文回读 | 仅事件字段 | 从 transcript 按轮次读取 | 完成通知可看全文 |
| Provider 表驱动 | 硬编码双元素 | 元数据表 + 设置字段映射 | 扩展不改 5 处 |

### 1.2 不做（并说明扩展点）

| 不做 | 理由 | 扩展点 |
| --- | --- | --- |
| Claude / ZCode / ChatGPT 接入 | 本机无对应客户端可实测，无法验收；盲目接入会产生「配置成功但永远不连接」的假状态 | `ProviderMeta` 表 + `SITES` 常量追加即可，见 §3.1 |
| usage 统计 | 与 daozai 现有 `quota` 模块职责重叠（后者读的是厂商额度接口）；Agent 事件用量属辅助信息 | 可在 `Cards` 增加计数，不影响主链路 |
| 会话历史与归档 | 违反 daozai「请求正文不落盘」隐私边界 | 如需，只保留不含正文的元数据 |
| 本地 HTTP 导航服务 | 违反 daozai「不开放网络 TCP 端口」约束 | 用 §3.4 的方案 A/C/D 替代 |
| 答案草稿持久化 | 会把用户答案写到磁盘，扩大隐私面；当前会话内已够用 | 如要做，必须限定只存答案不存正文，且可关闭 |

---

## 2. 架构设计

### 2.1 总览（保持现有链路，只做增量）

```
DSH 插件 / Codex hook
    │  (命名管道 + HMAC + NDJSON)
    ▼
channel.cjs  ──stdin/stdout NDJSON──  agent_cards.rs
    │                                    ├─ 队列与状态机
    │                                    ├─ 详情卡绘制（paint/theme）
    │                                    ├─ reply / hand_back
    │                                    └─ 新增：agent_jump / agent_reply
    ▼
transcript 文件（只读，可按轮次读取）
```

Rust 是 bridge worker 的父进程，worker 是管道服务端，插件是管道客户端。这条链路已有双向 HMAC、192 KiB 帧上限、0600 descriptor，**安全强度高于 island-hud，不改动**。

### 2.2 Rust 侧文件

| 文件 | 改动 |
| --- | --- |
| `src/agent_notice.rs` | `Provider` 改表驱动；`SITES` 追加；`Connection` 保持五态 |
| `src/agent_cards.rs` | 请求状态机扩展（`submitting`/`uncertain`/`expired`）；契约校验；终态处理 |
| `src/agent_jump.rs`（新增） | 跳转目标解析与执行 |
| `src/agent_reply.rs`（新增） | transcript 尾部读取（zstd + 轮次定位 + LRU） |
| `src/model.rs` | 新增设置字段（跳转开关、正文回读开关） |
| `src/extras.rs` | 设置界面接线；来源列表按 `ProviderMeta` 遍历 |
| `src/lib.rs` | 注册两个新模块 |

### 2.3 Node 侧文件

| 文件 | 改动 |
| --- | --- |
| `bridge/channel.cjs` | 新增 `receipt` 帧、`navigate` 命令帧、provider×kind 超时表 |
| `bridge/index.mjs` | DSH 事件补 `sessionId`/`cwd`/`turn` 透传 |
| `bridge/notify.cjs` | Codex 事件补 `sessionId`（从 hook payload 取） |
| `connect.mjs` | 不新增 provider 时无需改动 |

---

## 3. 详细设计

### 3.1 Provider 表驱动

替换 `Provider::ALL: [Self; 2]` 为元数据表：

```rust
pub struct ProviderMeta {
    pub provider: Provider,
    pub key: &'static str,          // 线上标识，wire_id 顺序由 SITES 固定
    pub label: &'static str,
    pub icon: fn() -> egui::Image<'static>,
    pub setting: fn(&crate::model::Settings) -> &bool,
    pub setting_mut: fn(&mut crate::model::Settings) -> &mut bool,
}
pub const SITES: &[ProviderMeta] = &[ /* dsh, codex 两项，顺序即 wire_id */ ];
```

约束：

- `SITES` 只能**追加**，不能重排，否则破坏 `wire_id` 兼容。
- `Provider::ALL` 由 `SITES` 派生，`agent_notice.rs`、`agent_cards.rs`、`extras.rs`、`tray.rs` 全部改为遍历 `SITES`。
- `Settings` 新增来源时，同时补 `model.rs` 的字段、默认值、`parse`/`save` 分支。

### 3.2 请求状态机

```rust
enum RequestState {
    Pending,        // 已入队，等待用户操作
    Submitting,     // 已发出决定，等待终态
    Uncertain,      // 提交失败或超时，锁控件保留草稿
}
```

迁移：

| 事件 | 迁移 |
| --- | --- |
| `receive` 收到 approval/question | → `Pending` |
| 用户点击允许/拒绝/回答/回客户端 | → `Submitting`，发出 reply，记录 `submitted_at` |
| 收到 `{receipt:'delivered'}` | → 移除请求，可选闪示回执 |
| 收到 `{receipt:'failed'}` 或 2 秒无回执 | → `Uncertain` |
| `Uncertain` 下再次点击 | 允许重试；`already_handled`/`expired` 直接移除 |
| `receivedAt+timeout` 到期 | → `Expired`，控件置灰，只提示一次 |

`Uncertain` 的恢复：收到同 id 的 `delivered` 即移除；用户点「回客户端」也移除（宿主会自己收口）。

### 3.3 契约校验（对齐 island-hud）

在 `receive` 反序列化后新增校验，失败一律 `hand_back`：

| 项 | 规则 |
| --- | --- |
| 题量 | 1–20 |
| 题干 | ≤20000 字符 |
| 选项数 | ≤50 |
| 选项 label | 非空、题内唯一 |
| 答案条数 | 必须等于题数 |
| 答案 id | 必须对应题目 |
| 单选 | 不允许 ≥2 选项，不允许选项与自定义并存 |
| 空答案 | `selected` 空且 `custom` 空 = 合法跳过 |

「空答案 = 跳过」必须允许：DSH 的部分题目可以不答，强迫回答会导致豆包式误答。

### 3.4 会话跳转

**数据来源**：DSH 事件补 `sessionId`、`cwd`、`turn`；Codex 补 `sessionId`（线程 id）与 `cwd`。

**解析顺序**（照搬 island-hud 的短路分级）：

```
codex 深链（exact） > 进程匹配（exact/high） > 客户端窗口（client） > 终端（workspace） > 目录兜底（workspace）
```

**执行方式**：

| 方案 | 做法 | 采用 |
| --- | --- | --- |
| A. Codex 深链 | `ShellExecuteW("open", "codex://threads/<id>")` | 是 |
| B. 出站请求 DSH | 只用 `ureq` 打 DSH 已监听的 127.0.0.1 端口，白名单 `^http://127\.0\.0\.1:\d+$` | 否（会话路由参数未实测，风险高） |
| C. 进程/窗口激活 | `SetForegroundWindow`（已有 `desktop.rs` 处理拒绝） | 是 |
| D. 目录兜底 | `explorer.exe <cwd>`（`main.rs:753` 已有写法） | 是 |
| E. 自建 HTTP 服务 | island-hud 的 `DshNavigationServer` | **永不实现**（违反无端口约束） |

**交互**：完成通知与审批详情底部增加「回到客户端」按钮，仅在存在可跳转目标时显示；跳转失败提示原因，不静默。

### 3.5 正文回读

**边界**：只读，不落盘，只在用户点「查看完整回复」时读一次。

**实现**：

1. 定位 transcript：由插件在 `finished` 事件中给出 `transcriptPath`（DSH 已有会话目录结构，需在 `index.mjs` 补字段）；拿不到路径就不做回读。
2. 读取：`zstd` 解码 JSONL 尾部（Cargo 已有 `zstd 0.13.3`），按 `turn` 定位最后一次 `assistant` 消息。
3. 上限：1 MiB 字节，超出截断并标记 `truncated`。
4. 缓存：内存 LRU 4 条 / 2 MiB，key 为 `(sessionId, turn)`，失效依据 `(size, mtime)`。
5. 状态：`Ok(text)` / `Unavailable` / `Expired`，对应三种文案。

**性能**：读取在后台线程，完成后 `ctx.request_repaint()`，不阻塞 UI 帧。

### 3.6 交互语义取舍（与 island-hud 的差异）

| 项 | island-hud | daozai 决策 | 理由 |
| --- | --- | --- | --- |
| 完成通知与请求 | 不排队，计时照走，过期即弃 | **保留 daozai 排队** | 不漏提醒更符合桌面工具定位；在设置说明写清 |
| 悬停是否暂停交回 | 不暂停 | **保留 daozai 暂停** | 用户把鼠标停在卡上通常是正在阅读，此时交回是反直觉的 |
| 下一轮门槛 | 无门槛 | **保留 daozai 门槛**，但补「跳过此题」 | 防漏答；同时允许显式跳过 |
| 自动交回取值 | 0–120 秒 | 保留 0/5/10/15/30/60/120 | 现有取值更易理解 |
| 回执胶囊 | 1.2 秒闪示 | 采用，用 `paint::capsule` 绘制 | 补上「操作已生效」的确认感 |

上述差异必须写进 `docs/AGENT-NOTICES.md`，避免后续维护者误判为 bug。

---

## 4. 分期计划

### 阶段 1：契约与状态机（低风险，先做）

**交付物**

- 契约校验全部落地（§3.3）。
- 两阶段提交与 `Uncertain` 态（§3.2）。
- 过期态与文案。
- Provider 表驱动（§3.1），行为不变，仅结构变化。

**涉及文件**：`agent_notice.rs`、`agent_cards.rs`、`model.rs`、`extras.rs`、`channel.cjs`

**验收**

- 新增 Rust 测试覆盖：选项超限回交、题干超限回交、单选互斥、答案条数不符拒绝、空答案跳过允许、提交后未回执进入 Uncertain、回执到达移除、过期置灰。
- `cargo test --lib agent_cards::` 与 `agent_notice` 全绿。
- `node --test tests/agent-notices.test.mjs` 全绿。
- 实机：DSH 发一道多轮题，能正常作答；Codex 审批能正常允许/拒绝。

**回滚**：状态机改动集中在 `agent_cards.rs`，回滚即恢复该文件；协议未变，插件无需回退。

### 阶段 2：会话跳转

**交付物**

- 新增 `agent_jump.rs`：解析与执行。
- DSH/Codex 事件补 `sessionId`/`cwd`。
- 完成通知与详情卡增加「回到客户端」。

**验收**

- Codex：点跳转能打开对应线程。
- DSH：点跳转能把 DSH 窗口带到前台；无目标时显示目录兜底或提示。
- 单元测试覆盖解析优先级与无目标分支。

**回滚**：跳转是纯新增，失败只影响按钮，不影响主链路；可临时隐藏按钮。

### 阶段 3：正文回读

**交付物**

- 新增 `agent_reply.rs`：zstd 读取 + 轮次定位 + LRU。
- `index.mjs` 补 `transcriptPath`。
- 完成通知「查看全文」走回读，带三种状态文案。

**验收**

- DSH 完成一轮后，能读到该轮 assistant 正文并显示。
- 读不到时显示「暂不可用」而非空白。
- 1 MiB 截断生效；不写任何文件。

**回滚**：设置项可关闭；关闭后回退到现有事件字段。

### 阶段 4（可选）：新增客户端

只在有实测客户端时做。按 `ProviderMeta` 追加一项 + `connect.mjs` 加一套安装逻辑 + 插件脚本，不改协议。

---

## 5. 风险与回滚

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| 表驱动重构引入回归 | 来源开关、图标、wire_id | 阶段 1 先做且行为不变，靠现有 8 个 Rust 测试 + 4 个新测试守住 |
| 两阶段提交导致卡片残留 | 用户看到已处理却仍在的卡 | 2 秒无回执即进入 Uncertain 并显示重试；`cancel` 帧仍立即移除 |
| 跳转打开错误窗口 | 用户困惑 | 只在置信度 exact/high 时自动执行；workspace 档只打开目录并说明 |
| transcript 格式变化 | 回读失效 | 只在用户主动点「查看全文」时读；失败降级为「暂不可用」，不影响通知 |
| 读取大文件卡顿 | UI 掉帧 | 后台线程 + 1 MiB 上限 + LRU |
| 误把正文写入日志 | 隐私泄露 | 回读路径禁止任何日志输出正文；测试断言只记元数据 |

**统一回滚点**：`main` 分支当前 HEAD `08250c5` + 未提交改动。每个阶段完成并验证后再进入下一阶段，任何阶段失败只回滚该阶段文件，不做 `reset --hard`。
