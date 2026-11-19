# island-hud 渲染层行为语义（daozai 移植参考）

调研对象：`D:\Codex\island-hud`。只记录**行为语义**（状态、条件、阈值、优先级、互斥），不记录颜色/像素/字体/动效曲线。
证据文件：`src/renderer/island.js`、`src/renderer/native-requests.js`、`src/renderer/presentation.js`、`src/renderer/question-drafts.js`、`src/app/{dsh,codex,claude,zcode}-integration-manager.cjs`；跨层结论补充定位到 `src/main.js`、`src/integrations/{contract.js,hub.js}`、`src/sessions/session-store.cjs`、`src/app/completion-reader.cjs`。
对照基线：daozai `src/agent_cards.rs`、`src/agent_notice.rs`、`src/extras.rs`（只读，未修改任何文件）。

---

## 1. 卡片／面板形态清单与优先级

渲染层是「单场景互斥」模型：任一时刻只有一个 `.layer` 处于 active（`island.js` `applyScene()` 里 `layer.classList.toggle('active', key === name)`）。场景名共七个：`compact / overview / all / jump / approval / ask / approved`（`island.js` `renderScene()`、`applyScene()`，初值见 `state.scene = 'compact'`）。

| 形态 | 出现条件 | 关键实现 |
|---|---|---|
| compact 摘要条 | 常驻兜底；原生请求被「收起」时也停在这里 | `renderCompact()` |
| overview 会话面板 | 悬停 150ms 自动展开（`expandOnHover`）或手动展开；`hero` 取第一个 waiting 会话，否则第一个 running，否则第一个会话 | `renderOverview()`、`DisplayPolicy.scene()` |
| all 全量会话面板 | 点「显示全部 N 个会话」；列表排序为「待处理优先 · 最近活跃在前」 | `renderAllSessions()`、`session-store.cjs` `snapshot()` |
| jump 完成结果卡 | 完成提示 `completionReveal === 'expanded'`，或手动 `setScene('jump')` | `renderCompletion()` |
| approval 原生审批卡 | 有待处理 `kind==='approval'` 请求且未收起 | `native-requests.js` `draw()` 审批分支 |
| ask 原生问答卡（含审阅子态） | 有待处理 `kind==='question'` 请求且未收起；子态 `reviewing` 为「检查答案」页 | `draw()` 问答分支、`reviewing` 分支 |
| approved 回执胶囊 | 用户做出决定且**当前无待处理请求**时闪示 1.2s | `confirmed()`、`resultTimer` |
| 只读预览卡 | 会话处于 `waitingForApproval/waitingForAnswer` 但岛内没有可操作的原生请求（只有日志态） | `renderReadOnly()`、`renderScene()` 中 `!native.owns(name)` 分支 |
| toast | 操作反馈，默认 3.2s；提交回执类减到 1.2s | `showToast()`、`confirmed()` |

场景优先级（`island.js` `desiredScene()`，自上而下短路）：
1. `native.scene()`——有原生请求时：收起态返回 `'compact'`，否则返回 `approval`/`ask`；
2. `'approved'` 且 `resultTimer` 仍在——回执胶囊优先于任何面板；
3. 有 waiting 会话且用户没有手动指定场景 → 对应 `approval`/`ask` 只读预览；
4. `DisplayPolicy.scene()` 结果：`manualScene`，否则 `completionReveal==='expanded' && completion` → `'jump'`，否则 `'compact'`。

结论：**原生请求永远压过面板与会话状态；完成结果卡只在既无请求又无 waiting 会话时才能上屏。**

---

## 2. 状态机

### 2.1 请求生命周期（跨层两段）

- 宿主侧状态机在 `src/integrations/hub.js`：`pending`（`pending.set`，含到期 `timer`）→ `submitting`（`respond()` 里 `rec.request.state='submitting'`）→ 终态 `delivered / failed / expired / already_handled / unavailable`（`finish()` 写入 `closed`，`closed` 上限 1000 条按插入序淘汰）。同一 `id` 重复到达直接回 `already_handled`（`addExternalRequest()`、`handleSocket()`）。
- 渲染侧 `native-requests.js`：`requests`（宿主快照数组）+ `active = requests[0]`（`sync()`）。附属标志：`collapsed`、`editing`、`composing`、`submitting`、`uncertain`、`reviewing`、`page`。
- 「结果未知」态：`api.reply` 返回 `failed` 或抛异常 → `uncertain = true`，`updateControls()` 把除「收起」外所有按钮/输入置灰，草稿保留，提示结果未知（`submit()`）。该态不会自行解除，只有宿主侧 `onConfirmation` 到达或 `sync()` 换请求（身份变化）才复位。
- 回执确认（两阶段提交）：`submit()` 把请求记入 `submitted`（`{at, confirmed:false, draftKey}`，10 分钟清理）；宿主 `island:native:confirmation` 事件按 `outcome` 映射提示（allowed-once / allowed-always / rejected / answered / cancelled / unavailable），且只在 10 分钟内、未确认过时接受（`connect()` 的 `onConfirmation`）。`answered` 校验通过后 `drafts.removeKey()` 清草稿。
- 收到请求列表更新的两种来源去重：`connect()` 先记 `initialRevision`，`api.snapshot()` 回来时若 `revision` 已变则丢弃快照，避免旧快照覆盖新事件（`connect()`）。

### 2.2 队列与优先级

- 队列就是宿主 `pending` Map 的插入序（到达顺序，FIFO），渲染层永远只展示 `requests[0]`（`sync()`）；头部显示 `1 / N 待处理`（`head()`、`updateControls()`）。
- **无抢占**：新到达的请求不会顶掉当前卡；当前请求结束后 `sync()` 换 `active`，并复位 `page=0 / reviewing=false / collapsed=false / editing=false / submitting=false / uncertain=false`（`sync()`）。
- 换身份前先 `save()` 旧请求草稿，并清空旧层 DOM（`sync()`）。
- **队列无条数上限**：`hub.js` 不对 `pending` 设上限；只有单请求超时兜底（`contract.js` `TIMEOUT_MS`：dsh 审批 120s、提问 600s；codex 6h；zcode/claude/chatgpt 24h；`addExternalRequest()` 把外部注入的 `timeoutMs` 夹在 5s–24h）。溢出策略因此是「到期自然淘汰」而非拒收。

### 2.3 完成提示与待处理请求的互斥

- 上屏互斥：`renderCompact()` 中 `const completion = !request && !waiting ? policy.completion?.session : null` —— 只要存在原生请求**或任一会话处于 waiting 阶段**，完成提示不上屏。
- 但计时不互斥：`DisplayPolicy.scene()` 每帧调用都会扣减 `completion.remaining -= delta`，仅当 `hovered || protected` 时暂停（`presentation.js`）。因此被请求占用期间完成提示的停留计时照样走完，`remaining <= 0` 即丢弃，**事后不会补看**。
- 判定「新完成」的门槛：`recordCompletion()` 要求会话有 `completedAt`、`seen` 去重键 `[id, turn, completedAt]`（上限 128）、且 `now - completedAt <= 5000`——启动恢复的旧完成不算新通知。
- 内容失效：会话 `turn` 变化或会话从列表消失时清空 `fullReply` 与一代 `fullGeneration`（`island.js` `onSessions()`）。

### 2.4 悬停／展开／详情对计时的影响

- `protectedState()`（`island.js`）：`native.protected() || composing || (非 compact 场景 && (存在文本选区 || 有 `details[open]` || 焦点在 TEXTAREA))`。
- 自动交回**不因悬停暂停**（`handoffIdle()` 只看 `collapsed/editing/composing/submitting/uncertain/reviewing` 与 `blocking`）；悬停只影响展开/收起策略与完成提示停留。
- 展开/收起由 `DisplayPolicy` 定时器驱动：悬停 150ms 后展开（`hoverAt`），移出 900ms 后收起（`leaveAt`），受 `protected` 阻断；定时器只在 `appVisible` 时建立 100ms 轮询（`updateTimers()`）。
- 文本域聚焦、输入法组合、请求详情展开都会经 `native.protected()` 参与 `protectedState()`，从而阻断自动收起与完成提示扣时。
- `window.blur` → `native.releaseEditing()`（`editing=false; composing=false; armHandoff()`），即失焦后交回倒计时**从整段时长重新开始**。

### 2.5 过期

- `remainingMs = receivedAt + timeoutMs - Date.now()`，`1s` 定时器仅在 `active && !collapsed && document.visibilityState !== 'hidden'` 时推进（`connect()` 的 `expiryTimer`、`tickExpiry()`）。**收起态不判断过期**，只能等宿主侧到期。
- 过期即 `active.state = 'expired'`，控件全灰，同一请求只提示一次「请求已过期，请返回客户端处理」（`expiryNotice`）。
- 到期文本优先级：有交回倒计时（`handoffDeadline > 0`）时显示交回文案，否则显示 `剩余 m:ss` / `已过期`（`tickExpiry()`、`expiryText()`）。

---

## 3. 多轮问答规则

- 题量与选项上限在契约层：1–20 题、每题选项 ≤50（`contract.js` `questionsOf()`）；题 `id` 必须唯一。
- **单选（`multiSelect=false`）**：选项互斥为 radio；点选选项会清空自定义文本（`draw()` 内选项 `change` 监听）；键入自定义文本会清空所有选项（`draw()` 内 `textarea` `input` 监听）。二者严格互斥，契约层再兜一次：单选不允许 `selected.length > 1`，也不允许同时有 `custom` 与非空 `selected`（`contract.js` `answersOf()`）。
- **多选**：选项为 checkbox，可多选；自定义输入语义是「补充内容（可选）」，**允许与选项共存**（`draw()` 里首题文案随 `question.options.length` 与 `multiSelect` 变化）；契约层对多选不做互斥校验。
- **"下一题"无门槛**：`click()` 中 `name === 'next'` 直接 `page++`，不校验本题是否已答；最后一题时按钮文案变「检查答案」，点击置 `reviewing = true`。首题左侧是「返回客户端」，`page > 0` 后变「上一题」（`page = Math.max(0, page-1)`）。
- **"提交回答"门槛**：仅"检查答案"页的确认提交走 `submit('answer')`；门槛在契约层——答案条数必须等于题数、`id` 必须对应、选项必须是该题合法 label、单选不得选项+文本并存（`contract.js` `answersOf()`）。**空 `selected` 且空 `custom` 是合法的「跳过此题」**，不注入默认答案（`answersOf()` 注释与实现）。
- 审阅页把每题渲染为「已选 + 自定义」拼接，未填写显示「未填写（将跳过此题）」（`draw()` 的 `reviewing` 分支）；「返回修改」置 `reviewing=false`，页码保留在最后一题；`Ctrl+Enter` 在审阅页直接提交（`keydown()`）。
- **答案保留**：草稿键 = `[provider, sessionId&&callId ? ['call',sessionId,callId] : ['request',id], questions]`（`question-drafts.js` `keyOf()`）；存储键 `islandHud.questionDrafts.v1`，TTL 24h，最多 32 条，写入 200ms 防抖，超限按插入序淘汰；载入时逐条校验类型与时间，损坏/不可用则退化为纯内存；题目数量或 `id` 序列与草稿不一致时整条丢弃并重建（`get()`）。
- 审批请求不产生答案草稿（`get()` 对 `kind !== 'question'` 返回 `[]`）。

---

## 4. 自动交回（handoff）

- 默认关闭：`presentation.js` `DEFAULTS.approvalHandoffSeconds = 0`，注释明确「0 = 一直等」；`normalize()` 夹在 `0..120` 整数秒。渲染层通过 `handoffSeconds: () => Number(settings.approvalHandoffSeconds) || 0` 读取（`island.js` 创建 `native` 时）。
- 可交回前提（`handoffIdle()`）：`active.blocking !== false` 且 `state === 'pending'` 且未 `collapsed/editing/composing/submitting/uncertain/reviewing`。**非阻塞提问（`blocking:false`，界面标注「可继续工作 · 待回答」）永不自动交回。**
- 起表：`armHandoff()` 在请求挂载、收起/展开、选项与文本变更、输入法结束、失焦释放编辑、切页时被重新调用；`seconds <= 0` 或非 idle 则清空计时并置 `handoffDeadline = 0`。
- 到点行为：`submit('return')`（等价于「返回客户端」决定），让宿主原生 UI 接手。
- 到点时若已不 idle（正在编辑等）：**不立刻交回**，改为按整段时长重新 `armHandoff()`，避免倒计时永久停摆（`armHandoff()` 注释与实现）。
- 文案语义：`N 秒后回客户端` / `已交回客户端`（`handoffText()`），并在过期标记位替换显示（`tickExpiry()`）。
- 宿主生命周期限制：请求被收起（`collapsed`）时不计交回；窗口隐藏时 `updateTimers()` 直接返回，但 `handoffTimer` 是独立 `setTimeout`，仍可能在后台上触发交回；宿主断开连接由 `hub.js` 的 `sock.on('close')` → `finish(...,'expired','host-disconnected')` 收口；`pagehide/beforeunload` 只做草稿落盘与清理（`native.destroy()`）。

---

## 5. 完成通知

- 档位：`completionView ∈ {markdown-summary, text-summary, full}`（`presentation.js` `choices`）。`markdown-summary` 用 `P.markdown(P.summary(text).text)` 渲染受限 Markdown；`text-summary` 用 `P.plain()` 纯文本段落。
- 容量：摘要上限 **1200 字符**（`presentation.js` `summary(text, limit = 1200)`，按 `Array.from` 计码点）。截断提示的触发条件有两个：摘要模式下原文 ≥1200 字符，或全文档模式 `reply.truncated === true`（`renderCompletion()` 的 `reply?.truncated || !full && Array.from(...).length >= 1200`）。
- 全文档：`api.completion({id, turn})` 异步取回，键 `[id, turn]` 去重 + `fullGeneration` 代数防过期写回；上限 **1 MiB**，超限置 `truncated: true` 并同样提示截断（`completion-reader.cjs` `LIMIT`、`read()`）。
- 全文档的三种非正常态都有明确文案：读取中「正在读取本回合完整回复…」、不可用「本回合完整回复暂不可用，请在客户端查看。」、宿主未给正文「客户端未提供结果正文。」（`renderCompletion()`）。
- 揭示方式：`completionReveal ∈ {glance, expanded}`——`glance` 只在 compact 条内联一行（`renderCompact()` 把 `已完成 · ` + 去空白后的纯文本当标题）；`expanded` 走 `jump` 面板（`DisplayPolicy.scene()`）。
- 停留：`completionDwellMs` 夹在 2000–15000，默认 5000（`DEFAULTS`、`normalize()`）；仅 `!hovered && !protected` 时扣时（`DisplayPolicy.scene()`）。结果区是独立滚动容器（`completion-body native-scroll`），代码中没有任何自动滚动定位逻辑（渲染层只有 `all` 列表会保持 `scrollTop`，见 `renderAllSessions()`）。
- 声音：渲染层**不发系统通知**（`src/renderer/*.js` 内无 `Notification` 调用），只有 WebAudio 提示音，`notificationSound ∈ {off, soft, full}`；触发条件是 `attentionPhase()` 变化且上一次非空——转入 waiting 阶段发 attention 音，转入 `completed` 发完成音（`applyScene()` 中 `lastAttention` 比较、`playNotification()`）。
- 结论：island-hud 的系统级通知完全在下游客户端/宿主侧，岛内不带正文；daozai 现有「同时发送系统提醒」明确不带正文（`src/agent_cards.rs` `Preferences::ui()` 文案、`src/extras.rs` `agent_finished()` 只发 `本轮已结束`）与之一致。

---

## 6. 集成状态提示

状态统一由各家 `status()` 计算。hook 型三家（codex/claude/zcode）字段一致：`installed / managed / connected / version / conflict / reason / lastSeen`；dsh 无 `conflict`，多出 `profileDir / timedQuestions / configuredTimedQuestions`（各自的 `status()`）。判定与含义：

- **未配置**：`state()` 为空且配置里没有受管条目 → `reason = 尚未安装 <X> 集成`（`dsh-integration-manager.cjs:105`、`codex:120`、`claude:116`、`zcode:111`）。配置文件不存在单独归入未配置（`ENOENT` 分支）。
- **已配置但未连接**：`installed === true` 且 `connected === false` → 提示「已配置，等待客户端触发/连接，首次需重启客户端」。分provider差异：
  - dsh：`installed` 要求 `dependencies[NAME]` 存在、`dsh.profile.bundles` 含该 bundle、符号链接指向真实 `index.mjs`（`status()`）；`configuredTimedQuestions !== timedQuestions` 时提示「问答模式尚未应用，请安装/修复后重启」；`hostVersion !== version` 时提示「配置已更新，请重启加载新版本」（`status()` 的 reason 链）。设置页另用 `presentation.js` `dshRepairPending()` 判定同一条件。
  - codex：额外要求四类 hook 全覆盖且含 `PermissionRequest`，提示「首次使用需在 Codex 内运行 `/hooks` 信任一次，审批卡片仅在交互会话出现」——**信任动作必须用户手动确认，岛不能代办**（`codex-integration-manager.cjs:118`）。
  - claude：`disableAllHooks === true` 单独判为阻断态并提示到设置里重新启用（`claude-integration-manager.cjs:109`）。zcode/claude/codex 另有「受管脚本目录缺失」「hook 配置不完整或被外部修改，请点击修复」分支。
- **已连接（即「已收到通知」）**：`connected = installed && lastSeen > 0 && now - lastSeen < 45000`（四家 `status()` 同构）。`lastSeen` 只由 `observe(envelope)` 更新，且各 manager 自己按 `envelope.provider` 过滤（`dsh:41`、`codex:41`、`claude:39`、`zcode:38`），事件来自 `main.js` `IntegrationHub({onChange,onEvent})` 中对四家 manager 的广播。**含义：45 秒内确实收到过该客户端的真实事件——「已连接」与「已收到通知」在 island-hud 是同一个判据，不是两个档位**（daozai 的 `Connection::{Configured,Seen}` 才是分开的两档，见 `src/agent_notice.rs`）。
- 异常优先：`transaction.json` 残留（上次集成操作未完成）优先级最高，提示点击修复恢复事务（`dsh-integration-manager.cjs:99`）；写入走 `operation.lock` + 事务日志，恢复函数 `recover()`。

---

## 7. 对 daozai 的迁移要点（egui 立即模式）

已有基础（`src/agent_cards.rs`）：队列上限 32、`deferred` 延后插入、`countdown()` 交回倒计时、`detail_size()` 动态高度、审批三键（回客户端/拒绝/仅允许这一次）与 `native_scope` 只读、`Data` 契约反序列化、`Preferences` 的 `handoff ∈ {0,5,10,15,30,60,120}`、`content ∈ {仅状态, 摘要 1200, 全文 32000}`、`dwell 2..15`。`src/agent_notice.rs` `Setup` 已有 `Off/Installing/Configured/Seen/Failed` 五态。

需要补齐的行为语义（按优先级）：

1. **两阶段提交与「结果未知」态**：daozai `Cards::reply()` 是"发出即移除"（乐观），失败后请求已消失；island-hud 用 `submitted` + `onConfirmation` 保留请求并区分 `delivered/failed/expired/already_handled`（`native-requests.js` `submit()`）。补齐后可避免"提交失败但卡片已消失"。
2. **过期状态与倒计时文案优先级**：daozai `countdown()` 只算交回，不判断 `receivedAt+timeout` 过期，也没有"收起态不判过期"的规则（`native-requests.js` `tickExpiry()`）。
3. **完成提示与待处理请求的互斥 + 停留计时**：daozai 用 `Alert` 队列（`src/extras.rs` `tick_alerts()`）把完成提醒排队，语义是"请求优先、完成排后"；island-hud 是"不排队、只互斥、计时照走、过期即弃"。二者不同，移植时要明确取舍——若要保留 daozai 的排队更贴合"不漏提醒"，但需要在文档里写明与 island-hud 的差异。
4. **悬停是否暂停交回**：daozai `countdown()` 在 `hovering || details || deferred || hidden` 时暂停；island-hud 悬停**不暂停**，只有收起/编辑/输入法/提交/未知/审阅暂停。按用户在岛上的操作习惯二选一并写进设置说明。
5. **多轮问答门槛**：daozai 用 `current_ready/all_ready` 计算"下一轮/提交回答"可用性（`draw_details()`），island-hud"下一题"完全不设门槛、只在契约层校验提交。daozai 的门槛更严，属可选增强，但需保证"跳过此题"仍可提交（island-hud `answersOf()` 明确允许空答案）。
6. **草稿缓存**：daozai 把答案存在 `Request.answers`（内存、随请求消失）；island-hud 有 24h/32 条/200ms 防抖的 localStorage 草稿缓存，键含 `callId` 以便宿主重放同一请求时恢复。daozai 若要支持重启后补答需要持久化，且必须遵守"正文不落盘"的隐私底线——**island-hud 落盘的是用户答案而非请求正文**，这条边界要照抄注释而非扩大范围。
7. **provider 扩展面**：island-hud 渲染层用 `presentation.js` 的 `providers/meta` 表驱动 chip 与按钮文案（5 家），未知 provider 有兜底。daozai 只有 `Provider::{Dsh,Codex}` 且 `icon()/label()` 是硬编码 match（`src/agent_notice.rs`），新增客户端时要先把这层改成表驱动。

Electron 特有、不能直接照搬的部分：

- `setTree()` 的 DOM 子树内容哈希跳过重绘、`ResizeObserver/requestAnimationFrame` 量尺寸并 `api.setBounds()` 通知主进程窗口尺寸——egui 立即模式每帧重绘，窗口尺寸由 `app.rs` 布局自行计算，这两层要丢弃，只保留"测量后把尺寸发布给宿主"的语义。
- 场景动效时长与曲线（`applyScene()` 的 `--motion-duration/--motion-curve`）、WebAudio 提示音（`playNotification()`）、`<details>` 原生折叠——daozai 用 `anim.rs`/`theme.rs` 现有体系与 `Cmd::Notify`，不要引入。
- `window.localStorage` 草稿与 `pagehide` 落盘（`question-drafts.js`、`native.destroy()`）——Rust 侧需换成文件或内存方案，并保留 TTL/条数/防抖语义。
- 渲染层的 `preview`/`demo` 两条假数据通路（`island.js` 顶部 `preview/isDemo` 分支、`demoData()`）是开发用，不需要移植；daozai 用 `ui_tests.rs` 直接构造 `json!` 请求（如 `h.app.agent_cards.receive(json!({...}))`）等价覆盖。
