# daozai 现状审计与差距清单

审计日期：2026-11-19
审计范围：`src/agent_notice.rs`、`src/agent_cards.rs`、`src/tray.rs`、`src/app.rs`、`src/extras.rs`、`src/model.rs`、`src/main.rs`、`src/ui_tests.rs`（Agent 相关部分）、`integrations/agent-notices/` 全部文件、`docs/PROJECT-HANDOFF.md`、`docs/AGENT-NOTICES.md`
参考基线：`D:\Codex\island-hud`（Electron 版，支持 5 个 provider）

---

## 1. 测试基线（本次实测）

| 测试 | 命令 | 结果 |
| --- | --- | --- |
| Agent Rust 测试 | `cargo test --lib agent_notice` | **8 / 8 通过** |
| Node bridge 测试 | `node --test tests/agent-notices.test.mjs` | **10 / 10 通过** |
| 编译基线 | `cargo check --lib` | 通过 |

Node 需使用 DSH 自带运行时（系统 PATH 中无 node）：
`%LOCALAPPDATA%\Programs\DeepSeek Harness\resources\runtime\primary-runtime\dependencies\node\bin\node.exe`

**环境限制说明**：本机测试进程运行在中完整性（0x1000），凡涉及 `Shell_NotifyIconW` 的测试会返回错误 5（拒绝访问），这是沙箱完整性差异导致，不是代码缺陷，已在托盘复刻任务中单独定位。

Rust 测试覆盖：来源接线与重试、Agent 固定消息解码、卡片控件可点击、队列跨 toast 存活并在断线时释放、提问在岛内作答、多轮提问逐轮显示与返回、来源开关与总开关组合。
Node 测试覆盖：DSH 安装与审批审计、Codex 合并/去重/移除、bundle junction 处理、非法配置与外部包保护、无岛时固定状态回传、一次性决策校验、continued 投影、并发隔离与隐私、Codex hook fail-open。

---

## 2. 当前能力矩阵

### 2.1 来源（provider）

| provider | 开关字段 | 图标 | 接入方式 | 交互插件就绪判定 |
| --- | --- | --- | --- | --- |
| DSH | `settings.dsh_notifications` | `assets/ai-icons/deepseek-harness.png` | `connect.mjs dsh install` 写 `%DSH_HOME%\profiles\desktop\package.json` | `dshReady` 事件且来源开启 |
| Codex | `settings.codex_notifications` | `assets/ai-icons/codex-app.png` | `connect.mjs codex install` 写 `%CODEX_HOME%\hooks.json` | 仅「已配置」，无独立 ready 通道 |

### 2.2 事件 × 动作

| 事件 | DSH | Codex | 岛上动作 |
| --- | --- | --- | --- |
| `finished` 轮次结束 | 支持 | 支持 | 仅通知，不进队列；内容档位 0/1/2 |
| `approval` 审批 | 支持 | 支持 | 仅允许这一次 / 拒绝 / 回客户端 |
| `question` 提问 | 支持（`user-questions/request`） | 只提示，作答在 Codex 原生界面 | 单选 / 多选 / 自定义 / 多轮 |
| native-scope 授权 | 支持展示 | 不适用 | 只能展示 / 拒绝 / 回客户端，不能允许 |

### 2.3 设置项（`Preferences`）

| 字段 | 含义 | 取值 |
| --- | --- | --- |
| `interactive` | 岛上处理审批与提问 | bool，默认 true |
| `handoff` | 自动交回 | 0(不交回)/5/10/15/30/60/120 秒，默认 0 |
| `completion` | 任务完成提醒 | bool，默认 true |
| `dwell` | 完成通知停留 | 2–15 秒，默认 5 |
| `expanded` | 轻提示 / 展开结果卡 | bool，默认 false |
| `content` | 完成内容 | 0 仅状态 / 1 回复摘要 / 2 本轮回复 |
| `deferred` | 接收延后提问 | bool，默认 false |
| `system_toast` | 同时发系统提醒 | bool，默认 false |

容量与门槛（`agent_cards.rs`）：

- 队列上限 **32**，超出直接回客户端。
- 提问数量 1–20，为空或超限回客户端。
- 完成内容摘要 **1200** 字符、全文 **32000** 字符。
- 单帧上限 **192 KiB**（`channel.cjs` `limit`）。
- Codex 操作正文 ≤ 24000 字符才允许「仅允许这一次」（`notify.cjs`）。

---

## 3. 缺陷与风险

### P0 — 会导致客户端卡住或行为错误

**3.1 未知 provider 静默丢弃，不回交客户端**

`src/agent_cards.rs:287`：

```rust
let provider=match v["provider"].as_str()?{"dsh"=>Provider::Dsh,"codex"=>Provider::Codex,_=>return None};
```

`?` 与 `_ => return None` 都会让 `receive` 直接返回，**不发送 `decision=return`**。后果：bridge 一旦发出本次版本不认识的关系（例如后续扩展 claude/zcode，或旧版岛仔遇到新版 bridge），请求会一直挂着，直到宿主自身超时。正确行为应与 `kind` 非法、`Data` 解析失败一致——立即回交。

同一函数中对照实现（正确）：

```rust
let Ok(data)=serde_json::from_value::<Data>(v["data"].clone()) else {self.hand_back(&id);return None;};
```

**3.2 选项数量与文本长度无上限校验**

`Question`/`Choice` 只反序列化，不校验：
- `options` 数量无上限（island-hud 限制 50）
- `question` 文本长度无上限（island-hud 限制 20000）
- `selected` 与 `custom` 无长度限制

`draw_details` 的循环对全部选项绘制，`detail_size` 只按 `min(6)` 估算高度。虽然 `body_clip` 会裁剪并支持滚动，但超长文本与超量选项会显著拖慢绘制并让滚动范围失真。需补与 island-hud 对齐的上限。

**3.3 没有按 provider 的超时约束**

island-hud 在 `integrations/contract.js` 中为每个 provider × kind 定义 `TIMEOUT_MS`（如 DSH approval 120 秒、Codex 6 小时）。daozai 没有对应约束，完全依赖宿主自身超时与 `handoff` 秒数。当 `handoff=0`（默认不交回）且宿主长期不撤销时，卡片可能长时间滞留。建议至少增加「最长展示时长」保护。

### P1 — 能力缺失（与 island-hud 对齐目标）

**3.4 完成通知正文只能来自事件本身**

`receive` 对 `finished` 只取 `data.text` 并按字符截断。island-hud 有 `CompletionReader` 从 transcript 尾部按轮次读取，支持 LRU 缓存、分片扫描、`unavailable/expired` 状态。daozai 的 `channel.cjs`/`index.mjs` 当前没有读取 transcript 的能力，所以：
- 宿主未在事件里带 `last_assistant_message` 时只能显示「仅状态」。
- 无法表达「正文不可用」与「正文已过期」的区别。

**3.5 没有会话标识，无法跳转**

`Data` 结构没有 `sessionId`/`cwd`/`pid` 之外的会话身份，`Request` 也不保存。因此无法实现「点击岛卡回到原会话」。island-hud 通过 `resolveJumpTarget` 按 process → client → terminal → workspace 四级降级，并为 Codex 提供 `codex://threads/<id>` 深链。

**3.6 没有 usage / 额度统计**

island-hud 有 `usage-store.cjs` 按 provider 聚合。daozai 的 `quota` 模块是另一套（读各厂商额度接口），与 Agent 事件无关，不能替代。

**3.7 没有会话历史与归档**

island-hud 有 `session-store.cjs`、`session-index.cjs`、`archive-store.cjs`。daozai 明确不保留请求正文历史（隐私约束），所以这一块应**选择性不做**，仅可考虑保留不含正文的元数据（时间、provider、类型、结果）。

**3.8 只有 2 个 provider**

island-hud 的 `ACTIVE_PROVIDERS` 为 `['dsh','zcode','codex','claude','chatgpt']`，且 `integrations/` 下有四套 hook 实现。daozai 只有 DSH 与 Codex。

### P2 — 工程质量

**3.9 Provider 硬编码为双元素数组**

`src/agent_notice.rs:9`：

```rust
pub const ALL: [Self; 2] = [Self::Dsh, Self::Codex];
```

扩展 provider 需要同时改 5 处：

| 文件 | 位置 | 改动 |
| --- | --- | --- |
| `src/agent_notice.rs` | `Provider::ALL`、`id()`、`label()`、`icon()`、`enabled()` | 新增枚举分支 |
| `src/agent_notice.rs` | `SIGNALS` 常量 | **必须追加在末尾**，保持 wire_id 顺序 |
| `src/agent_cards.rs` | `receive` 的 provider 字符串映射、`update` 的 `[dsh,codex]` 解构 | 改为按 ALL 迭代 |
| `src/extras.rs` | `match p { Dsh=>..., Codex=>... }` 取设置字段 | 新增分支 |
| `src/model.rs` | `dsh_notifications`/`codex_notifications` 字段、默认值、读写、序列化 | 新增字段 |
| `src/tray.rs` | 依赖 `SIGNALS` 的 wire_id 测试 | 补用例 |

`update` 中的解构写法 `let [dsh,codex]=Provider::ALL.map(...)` 在 provider 数量变化时需要同步改，是脆弱点。

**3.10 单帧上限 192 KiB 偏紧**

island-hud 为 512 KiB。daozai 的 192 KiB 来自 `channel.cjs`，而 Codex 操作正文允许到 24000 字符（约 72 KB 中文），加上其他字段仍有空间，但若未来支持更大正文会触顶。当前不构成缺陷，扩展时需一并评估。

**3.11 详情卡高度按字符数估算，中英混排不准**

`detail_size` 用 `chars().count()/48`、`/54`、`/30` 等固定除数估算行数，中英文宽度差异大，可能低估导致内容挤压。已有 `clamp(214.0, WIN_H-8.0)` 兜底，不会超出窗口，但视觉上仍可能拥挤。

---

## 4. 文档一致性核对

| 文档表述 | 实际 | 结论 |
| --- | --- | --- |
| 「Agent 相关 Rust 测试已核验 8/8 通过」 | 8/8 通过 | 一致 |
| 「Node bridge 测试：10/10 通过」 | 10/10 通过 | 一致 |
| 「release 哈希 186D9409…」 | 已被本轮托盘修复更新为 CE5125F1… | **文档已过期，本轮已修正** |
| 「桌面快捷方式指向 D:\Codex\daozai\target\...\release\daozai.exe」 | 已改为 `C:\Users\Administrator\Daozai\daozai.exe`（脱离沙箱目录） | **文档已修正** |
| AGENT-NOTICES.md 描述的三档内容策略、停留 2–15 秒、默认不自动交回、队列语义 | 与代码一致 | 一致 |
| AGENT-NOTICES.md 未提及「未知 provider 会静默丢弃」 | 代码确实不回交 | **文档未覆盖该缺陷** |

---

## 5. 三类清单

### 5.1 可复用（无需改动）

- `integrations/agent-notices/bridge/channel.cjs`：NDJSON + nonce/HMAC 双向握手 + 命名管道 + descriptor 清理，与 island-hud 的 broker 等价且满足 daozai 隐私约束。
- `integrations/agent-notices/connect.mjs`：备份 `.bak` + 原子 rename + 外部修改检测 + 冲突保护（拒绝覆盖他人包/hook）。与 island-hud 的 integration-manager 相比更保守，应保留。
- `agent_notice::export`：把内置文档与脚本导出到 `%APPDATA%\Daozai\agent-notices`，含 `target.json`。
- `agent_notice::Setup` 状态机：`Off/Installing/Configured/Seen/Failed` 与重试逻辑，测试已覆盖。
- `agent_cards::Preferences::normalized` 的取值收敛。
- 多轮问答的 `question_index`/`answers`/`question_scroll` 状态模型（UI 测试已覆盖逐轮、返回、互斥、提交门槛）。
- `countdown` 的暂停条件（hover / details / deferred / hidden）。
- Rust ↔ Node 的 policy 下发机制（`update` 中只在变化时发送）。

### 5.2 需改造

| 项 | 现状 | 改造方向 | 对应 island-hud 参考 |
| --- | --- | --- | --- |
| 未知 provider 处理 | 静默 return | 改为一律 `hand_back` | `contract.js` `validateRequest` |
| 输入校验 | 仅 questions 数量 | 补 options ≤50、文本长度、answers 结构校验 | `contract.js` `questionsOf`/`answersOf` |
| Provider 扩展性 | 硬编码 `[Self;2]` | 改为可迭代 + 设置字段表驱动 | `contract.js` `ACTIVE_PROVIDERS` |
| 完成正文来源 | 仅事件字段 | 增加可选的 transcript 尾部读取（受隐私约束限制） | `completion-reader.cjs` |
| 超时保护 | 无 | 增加最长待处理时长与到期交回 | `contract.js` `TIMEOUT_MS` |
| 详情卡高度 | 字符数估算 | 用 egui 实测文本布局高度 | `presentation.js` 的量测思路 |

### 5.3 缺失

| 能力 | 说明 | 对应 island-hud 参考 |
| --- | --- | --- |
| 会话标识与跳转 | 无 sessionId，无法回到原会话 | `jump-target.cjs`、`dsh-navigation.cjs` |
| Claude / ZCode / ChatGPT 接入 | 只有 DSH、Codex | `integrations/claude`、`integrations/zcode` |
| usage 统计 | 无按 provider 的 Agent 用量 | `usage-store.cjs` |
| 正文不可用/过期状态 | 无法区分「没有正文」与「读取失败」 | `completion-reader.cjs` 的 `status` |
| 多轮提问草稿 | 切走再回来时保留未提交答案（当前切轮保留，关闭详情后是否保留需确认） | `question-drafts.js` |

---

## 6. 结论

daozai 现有的 Agent 能力在**协议安全性与接入可靠性**上已经达到甚至略优于 island-hud（HMAC 握手、更保守的冲突保护、更严格的 fail-open 语义、隐私不落盘），测试覆盖也扎实。

真正的问题是三类：

1. **一个可导致客户端卡住的真实缺陷**（未知 provider 不回交），必须修。
2. **输入校验不完整**，与 island-hud 的 contract 层存在差距，需补齐上限。
3. **能力面窄**：只有 2 个 provider、无会话跳转、无正文回读、无 usage，这是「复刻」的主要工作量所在。

因此复刻策略应是：**先修 P0 缺陷并补齐契约校验（低风险、高收益），再按 provider 扩展与会话跳转分两个阶段推进**，而不是一次性重写。
