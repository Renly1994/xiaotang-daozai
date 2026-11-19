# island-hud 接入层与协议契约（调研报告）

范围：`D:\Codex\island-hud` 的接入层与线协议。只读调研，未修改任何文件。
依据文件：`src/integrations/{contract,hub,client}.js`、`src/bridge/bridgeServer.js`、`integrations/dsh/{index.mjs,lib/contract.cjs,lib/client.cjs,cordis.patch.yml,package.json}`、`integrations/codex/{lib/contract.cjs,lib/client.cjs,scripts/island-hook.cjs,package.json}`；hook 注册部分补读了 `src/app/codex-integration-manager.cjs` 的安装函数（任务第 5 项要求，仅该类）。

约定：`integrations/*/lib/contract.cjs` 与 `lib/client.cjs` 均由 `tools/build-integrations.cjs` 从 `src/integrations/` 自动生成（文件首行注释），内容与源文件逐字相同，下文只标注源文件。

---

## 1. 事件契约

### 1.1 kind 只有三种
`src/integrations/contract.js` → `validateRequest()`：`kind` 必须属于 `['approval','question','event']`，否则 `throw new Error('Invalid envelope')`。没有 `finished`/`ready` 这类 kind——完成态在 island-hud 里是 `kind:'event'` + `payload.event`（如 `Stop`、`QuestionAnswered`），`integrations/dsh/index.mjs` → `report()` 就统一这样发。

### 1.2 线上一帧（客户端 → 岛）允许出现的字段
`validateRequest()` 读取的原始字段与限制：

| 字段 | 类型/约束 | 必填 | 失败行为 |
|---|---|---|---|
| `provider` | 属于 `ACTIVE_PROVIDERS`（5 种） | 是 | 抛错，hub 直接 `sock.destroy()` |
| `id` | string，非空，≤500 字符 | 是 | 同上 |
| `kind` | `approval`/`question`/`event` | 是 | 同上 |
| `sessionId` | string，≤500；允许 `undefined`/`null` | 否 | 抛错 |
| `payload` | object，未知字段原样保留（`{...msg.payload}`） | 否 | 不校验 |
| `surface` | string，≤120 → 保留；超长/非 string → **静默丢弃**，不报错 | 否 | 不报错 |
| `blocking` | 任意，`!== false` 即为 `true` | 否 | 不报错 |
| `at` | 仅 `event` 有效；`Number.isSafeInteger` 且 `>0` 且 `<= Date.now()+1000` 才采用，否则取 `Date.now()` | 否 | 静默回落 |

### 1.3 归一化后的请求对象（岛内部用）
`validateRequest()` 返回 `{provider, [surface], requestId, id, sessionId, sessionKey, kind, blocking, payload, at, timeoutMs, receivedAt, state:'pending'}`：
- `requestId` = 线上 `id`；`id` = `keyOf(provider,id)` = `JSON.stringify([provider,id])`，即岛内主键带 provider 前缀，避免跨 provider 撞 id（`contract.js` → `keyOf()`）。
- `sessionKey` 由 `sessionKey()` 生成：`sessionId` 为空时是 `null`，否则 `keyOf(provider,sessionId)`。
- `state` 初始 `'pending'`，提交时改 `'submitting'`（`src/integrations/hub.js` → `respond()`）。

### 1.4 question payload 的强制规整
`kind==='question'` 时 `payload.questions` 必须通过 `questionsOf()`，输出被裁剪为白名单字段：`{id, question, detail:String(..||''), multiSelect:!!, options:[{label, description:String(..||'')}]}`。上限见第 4 节。

### 1.5 approval / event payload 无字段校验
`validateRequest()` 对这两类 payload 不做任何字段/长度检查，只有 512KiB 帧上限兜底。实际形状由各客户端自定：
- DSH：`integrations/dsh/index.mjs` → `approval/request` 处理器构造 `{cwd, sessionTitle, callId, tool:{name, detail, reason, target?, operation?}}`；`user-questions/request` 处理器构造 `{cwd, sessionTitle, questions, callId, timed, continuation}`。
- Codex：`integrations/codex/scripts/island-hook.cjs` → `approvalPayload()` 构造 `{cwd, sessionTitle, callId, tool:{name, detail, reason, target?, operation?}}`，其中 `operation` 是 `tool_input` 的 JSON 截断到 `MAX_INPUT_JSON=600` 字符，`target` 截 200，`detail` 截 160，`reason` 截 500。

---

## 2. provider 集合与差异

- `contract.js` → `ACTIVE_PROVIDERS = ['dsh','zcode','codex','claude','chatgpt']`（冻结数组），`PROVIDERS` 是其 Set 形式；`ValidateRequest` 只认这 5 个。
- `contract.js` → `TIMEOUT_MS` 分档（毫秒）：

| provider | approval | question |
|---|---|---|
| dsh | 120000（2 分钟） | 600000（10 分钟） |
| codex | 6h（21600000） | 6h |
| zcode / claude / chatgpt | 24h（86400000） | 24h |

- `contract.js` → `timeoutFor(provider,kind)`：非 `approval`/`question` 一律返回 `0`（即 event 不设等待超时）；未知 provider 回落 `TIMEOUT_MS.dsh`。
- 差异根因写在注释里：ZCode/Claude Code/Codex 的 hook 客户端会**阻塞 agent 自己的工具调用**直到岛上的卡片关闭，所以可以等几小时；DSH 是宿主内流程，只能在宿主的前台截止时间内完成。
- 阻塞语义并非协议字段强制，而是各客户端自己实现的：
  - DSH（`integrations/dsh/index.mjs`）：`ctx.on('approval/request', ...)` 用 `prepend:true` 抢占，`invoke()` 同步等结果；`delivered + allow` → 返回 `'allowed-once'`，`delivered + deny` → `'rejected'`，其余（return/超时/不可用）→ `return next()` 交回宿主原生流程。question 同理，只有 `delivered + answer` 才返回 `{answers}`。
  - Continued question（宿主前台已结束的追问）：`scanContinued()` 以 `blocking:false` 发起，每 1000ms 轮询投影状态，失败/不可用退避 3000ms，`dispose` 时全量 abort。
  - Codex（`integrations/codex/scripts/island-hook.cjs`）：进程阻塞在 `await exchange(...)`，最终把决定写 stdout；任何异常都 fail-open。
- 岛的 provider 开关：`hub.js` → `IntegrationHub` 构造函数 `allowedProviders` **默认只有 `['dsh']`**；不在名单内的连接在通过握手后立刻收到 `{status:'unavailable', reason:'provider-disabled'}` 然后关闭（`handleSocket`）。这是"来源开关"的落点。
- `hub.js` → `addExternalRequest()`（给宿主内流程用的旁路入口）会把传入 `timeoutMs` 夹到 `[5000, 24h]`，并在超时时先回调 `externalRespond({decision:'return'})`、再 `finish(...,'expired')`。

---

## 3. 通道协议

### 3.1 传输载体：命名管道，非对外 TCP
- `src/bridge/bridgeServer.js` → `start()`：`pipeName = island-hud-<进程pid>-<4字节hex>`，`pipePath = \\.\pipe\<pipeName>`；同时在 `127.0.0.1:0`（随机端口）起一个 TCP server，只绑 loopback，供内置 C# helper 转发。
- `startHelper(port)`：spawn helper，参数顺序 `(pipeName, port, pid)`；helper 用显式 per-user DACL 建管道，打印 `READY` 后才 `writeDescriptor()`。5 秒未 READY 则 kill 并回落。
- `startDirectPipe()`：helper 不存在或退出时的兜底，直接用 Node 建管道，**没有自定义 ACL**，日志原文为 `using direct named pipe (no custom ACL)`。这是安全性依赖 helper 的关键点。
- 客户端只连管道：`src/integrations/client.js` → `exchange()` 里 `net.createConnection('\\\\.\\pipe\\' + d.pipe)`。

### 3.2 帧上限与分帧
- 上限常量 `contract.js` → `MAX_FRAME_BYTES = 512 * 1024`（512KiB）。
- 分帧统一为 NDJSON：一行一帧，以 `\n` 结束。
- 客户端：发送前 `Buffer.byteLength(frame) > MAX_FRAME_BYTES` → 断连并返回 `{status:'failed', reason:'frame-too-large'}`；接收缓冲累计超限 → `invalid-frame`（`client.js` → `exchange()`）。
- 服务端：`sock.on('data')` 里累计 buffer 超 `MAX_FRAME_BYTES` 直接 `destroy()`；任一帧 `JSON.parse` 失败也 `destroy()`（`hub.js` → `handleSocket()`）。
- 注意上限是**累计缓冲**而不是单帧长度：连续小帧不换行也会触发。

### 3.3 握手与认证
- 握手帧 = 第一帧，必须 `{v:2, token:<descriptor里的token>, ...envelope}`。
- 服务端（`hub.js` → `handleSocket()`）：建 socket 后起 2000ms 定时器，超时未完成握手 → `sock.destroy()`；`msg.v !== 2 || msg.token !== this.token` → `destroy()`。握手成功后 `clearTimeout`。
- token 产生：`bridgeServer.js` → `start()` 里 `crypto.randomBytes(24).toString('hex')` = 192 位 / 48 hex，**每次启动新生成**。
- pid 与 SID 相关约束：
  - pid 写进 descriptor 供排障和停止时判断归属（`stop()` 里读 descriptor，`d.pid === process.pid` 才删除）。
  - "SID 限制"不在 JS 层实现，而在 helper 创建的管道 DACL（每用户）；descriptor 侧由 `bridgeServer.js` → `hardenDescriptor()` 用 `whoami /user /fo csv /nh` 取当前用户 SID，再 `icacls <file> /inheritance:r /grant:r *<SID>:F *S-1-5-18:F *S-1-5-32-544:F`，即只留当前用户 + SYSTEM + Administrators。失败只记日志，不影响服务。
- 客户端对 descriptor 的校验（`client.js` → `exchange()`）：`d.v===2`、管道名匹配 `/^island-hud-\d+-[a-f0-9]+$/`、token 匹配 `/^[a-f0-9]{48}$/`；任一不满足 → 立即 `{status:'unavailable', reason:'bridge-not-running'}`。
- 连接超时：`Math.min(connectMs, 1800)`，默认 `connectMs=1500` → 1500ms。

### 3.4 descriptor 文件
- 路径：`%LOCALAPPDATA%\IslandHUD\integration-v2.json`，可用环境变量 `ISLAND_HUD_BRIDGE_FILE` 覆盖（`client.js` → `descriptorPath()`）。
- 格式（`hub.js` → `writeDescriptor()`）：`{"v":2,"pipe":<管道名>,"token":<48hex>,"pid":<宿主pid>,"createdAt":<启动ms>}`。
- 写入方式：先写 `<path>.tmp` 再 `renameSync` 覆盖（原子替换，避免读到半个文件），随后调用 `hardenDescriptor()` 收紧 ACL。
- 关键事实：**token 在 descriptor 里是明文**。任何能读该文件的同用户进程都能拿它连上岛并伪造请求；防护完全依赖 descriptor 的 ACL。

### 3.5 一连接一请求
`hub.js` → `handleSocket()` 用闭包变量 `request` 标记：收到第一帧（握手）即当作该连接唯一的请求；此后再来的帧只被当作 `receipt` 处理，其他形状一律 `sock.destroy()`。

### 3.6 回执（receipt）与终态
- 岛投递决定：`hub.js` → `respond()` 写 `{v:2, type:'decision', provider, id, decision, ...extra}`，然后起 **2000ms** 的 `receiptTimer`；超时 → `finish(id,{status:'failed', reason:'adapter-receipt-timeout'})` 并 `destroy()`。
- 客户端收到 decision 后，先跑自己的 `validate`，再回 `{v:2, type:'receipt', id, provider, status}`（`client.js` → `exchange()`）。status 白名单 `delivered`/`already_handled`/`expired`，其他一律记 `failed`（`hub.js` → `handleSocket()`）。
- 语义边界写在 `client.js` 注释里：**receipt 只表示"决定已交给适配器"，不表示宿主已执行或已验证**。

### 3.7 断线与超时语义
- 岛侧超时：`hub.js` → `handleSocket()` 为每个 pending 起 `setTimeout(..., request.timeoutMs)`，到点先回 `{status:'expired'}` 再 `finish(id,{status:'expired'})`。
- 断线：`hub.js` → `sock.on('close')` 时若该 socket 仍持有 pending，`finish(id,{status:'expired', reason:'host-disconnected'})`。
- 客户端侧：`error` → `unavailable/connection-lost`；`close` → `connected ? 'disconnected' : 'connect-failed'`；自身等待超时（`timeoutFor` 值，event 回落 1500ms）→ `expired`；`AbortSignal` → `cancelled`。
- 岛停止：`hub.js` → `stop()` 把所有 pending 以 `{status:'unavailable'}` 结束并销毁所有 socket。
- 去重与幂等：`pending` 已有该 id 或 `closed` 里有 → 回 `{status:'already_handled'}`；`closed` 是最近 1000 条结果的环形表（`finish()` 里 `while (this.closed.size > 1000) delete 最旧`）。

### 3.8 兼容层说明
`bridgeServer.js` 仍保留 v1 老协议：descriptor `bridge.json`、`VERSION=1`、只有 `approval`/`question`、`DEFAULT_TIMEOUT_MS=60s`、`timeoutMs` 夹到 `[5s,300s]`、`respond()` 直接回 `{v:1,id,decision,...extra}`。`IntegrationHub` 通过覆写 `writeDescriptor()`/`handleSocket()` 走 v2，v1 仅作为传输基座（`handleMessage()` 已不在 v2 路径上）。

---

## 4. 校验函数行为

统一的错误模型：**校验函数只会 `throw Error`，异常在调用点被转成"断连"或"status"**，协议里没有错误码帧。

### 4.1 `contract.js` → `questionsOf(input)`
拒绝并抛错：
1. 非数组、空数组、`length > 20` → `Expected 1–20 questions`（题目数上限 20）。
2. 某题 `id` 非 string、空白、或与已出现的重复 → `Question IDs must be unique`。
3. `question` 非 string、空白、或 `length > 20000` → `Invalid question text`（单题正文 20000 字符）。
4. `options` 存在但非数组、或 `length > 50` → `Invalid options`（每题选项上限 50）。
5. 选项 `label` 非 string、空白、或重复 → `Option labels must be unique`。
不做校验：`detail`/`description` 非 string 会被 `String()` 强转（`null`→`"null"` 这类是已知副作用）。

### 4.2 `contract.js` → `answersOf(questions, input)`
拒绝并抛错：
1. 非数组，或 `input.length !== questions.length` → `Answer every question`（必须逐题给答案，不能少不能多）。
2. 条目为空、`id` 重复、`selected` 非数组、`selected` 含非 string、`custom` 存在但非 string → `Invalid answer batch`。
3. 找不到对应题、`selected` 内有重复 label、label 不在该题 `options` 里 → `Invalid option or question ID`。
4. 非多选时 `selected.length > 1`，或 `custom` 与 `selected` 同时存在 → `Choose one option or custom text`（单选与自定义互斥）。
- 通过后按 `questions` 顺序返回 `[{id, selected:[...]}, ...(custom?{custom}:{})]`；`selected` 为空数组是合法的，注释明确写"代表跳过 DSH 题目，绝不臆造默认答案"。
- 未知 id（不在题目里的答案）会被忽略：返回时按 `questions.map` 重建，多余条目自然丢失。

### 4.3 `contract.js` → `validateRequest(msg)`
拒绝并抛错：`provider` 不在 5 种内、`id` 非 string/空/>500、`kind` 不在 3 种内、`sessionId` 存在但非 string 或 >500。
不拒绝：未知 payload 字段、`surface` 超长（静默丢）、`blocking` 任意值、`at` 非法（静默回落）。

### 4.4 `contract.js` → `validateDecision(request, decision, extra)`
- `decision === 'return'` → 恒返回 `{decision:'return'}`（"交回"，无需任何其它参数）。
- `kind==='approval'`：只接受 `allow`/`deny`/`allow-always`，否则 `Invalid approval decision`。
- `kind==='question'`：只接受 `'answer'`，否则 `Invalid decision`；通过后必须再过 `answersOf()`，返回 `{decision:'answer', answers:[...]}`。
- `kind==='event'`：除 `return` 外全部抛错（event 本身没有决定）。

### 4.5 错误在调用点如何处理
| 调用点 | 处理 |
|---|---|
| `hub.js` → `handleSocket()` 首次 `validateRequest` | `catch { sock.destroy(); return; }`，客户端只看到断连 |
| `hub.js` → `handleSocket()` 收到 `already_handled` 等分支 | 回一帧 status 后关闭 |
| `hub.js` → `respond()` 的 `validateDecision` | `catch (e) => ({status:'failed', reason:e.message})` **只回给岛内渲染层**，不写回客户端 |
| `hub.js` → `addExternalRequest()` 的 `validateRequest` | `catch { return null; }`，静默丢弃 |
| `client.js` → `exchange()` 的 `validate` 回调 | 发 receipt（`already_handled`/`expired` 透传，其余 `failed`），resolve `{status, reason}` |
| `integrations/dsh/index.mjs` → `approval/request` 的 validate | 决定不在 `allow/deny/return` 内 → 抛 `Invalid approval`，等价于一次失败并交回宿主 |
| `integrations/codex/scripts/island-hook.cjs` → `handlePermission()` 的 validate | 决定不在白名单 → 抛错，被外层 `catch` 吞掉后 fail-open 退出 0 |

---

## 5. 接入脚本（hook 注册 / 防冲突 / 备份 / 退出码）

### 5.1 两种完全不同的接入方式
- **Codex：文件 hook。** `integrations/codex/package.json` 声明 `main: scripts/island-hook.cjs` + `islandHud.owner: "island-hud-codex-integration-v1"`。由岛的 `src/app/codex-integration-manager.cjs` → `CodexIntegrationManager.install()` 写 `~/.codex/hooks.json`。
- **DSH：插件 bundle patch，无文件 hook。** `integrations/dsh/cordis.patch.yml` 只有一条 `insert: [{id: island-hud-native-bridge, name: '@island-hud/dsh-bridge'}]`，靠 `package.json` 的 `dsh.bundle.patch` 被宿主加载；`index.mjs` 通过 `ctx.on('approval/request'|'user-questions/request'|'session/event', ...)` 挂钩。

### 5.2 注册的事件与超时
`codex-integration-manager.cjs` 顶部常量：
- `EVENT_HOOKS = ['SessionStart','UserPromptSubmit','PreToolUse','PostToolUse','Stop']`，`EVENT_TIMEOUT_S = 10`（秒）。
- `PermissionRequest`，`PERMISSION_TIMEOUT_S = 6 * 3600`（秒，即 6 小时，与 `TIMEOUT_MS.codex` 对齐）。
- Codex 的 hook 超时单位是**秒**，且条目必须有 `command` 字段，Windows 用 `commandWindows`（通过 `cmd /C` 执行）——这是 `hookEntries()` 注释里点明的坑。
- 条目形状（`hookEntries()`）：`{type:'command', command:'node "<脚本路径>"', commandWindows:<同上>, timeout:<秒>, statusMessage:'Island HUD'}`，路径里的 `\` 统一换成 `/` 以便带空格的路径能被引号包裹。

### 5.3 防冲突
1. **所有权标记**：`isOwnedEntry(entry)` 把条目 `JSON.stringify` 后小写、把 `\\` 归一成 `/`，要求同时包含 `ownedMarker` 与 `'island-hook.cjs'`。`ownedMarker` 是 `integrations/codex/packages` 目录路径（**不带版本号**），这样升级换了内容寻址目录也不会留下孤儿条目（构造函数注释说明）。
2. **升级清理**：`install()` 里对比每个事件的 `owned` 组，与当前应写组不同的（`stale`）先删除再补写，避免每次升级堆积多组。
3. **与他人 hook 共存**：`detectConflict(config)` 只看 `PermissionRequest` 事件；若有非本方的组，取第一个 `commandWindows||command||type` 截 80 字符，返回提示"检测到其他 PermissionRequest hook（…）；双方都会等待审批响应，请按需停用其一。"——只提示，不自动删除别人的条目。
4. **并发保护**：`withLock()` 用 `<home>/install.lock`（`wx` + `0o600`），已在则读 pid 并用 `process.kill(pid, 0)` 探活，存活就抛"另一个 Codex 集成操作正在进行"，`ESRCH` 视为残留锁直接删除重试。
5. **提交前乐观并发**：`writeConfig()` 重新读当前文件字节，与进入时的 `before` 逐字节比较，不一致抛"Codex 配置在操作期间被修改；未应用任何改动，请重试"。
6. **配置文件容错**：`readConfig()` 拒绝 >2MiB 文件、BOM 会剥掉、`hooks` 非对象或某事件不是数组时抛"Codex 配置结构不受支持"；未知顶层键原样保留。

### 5.4 备份策略
`writeConfig(before, next)`：备份目录 `<dataDir>/backups/codex/<毫秒时间戳>-<uuid>/`，把原 `hooks.json` 的**原始字节**以 `{flag:'wx', mode:0o600}` 写入；路径记录在 state 文件里随安装结果返回（`backupDir`）。`remove()` 同样先备份再改，且注释写明"备份已保留"——备份不自动清理。

### 5.5 部署产物完整性
`prepareBundle()` 把 `scripts/island-hook.cjs`、`lib/client.cjs`、`lib/contract.cjs`、`package.json` 四个文件算 sha256，落到 `integrations/codex/packages/<version>-<sha256前20>`；已存在且字节不一致就抛"受管 Codex 脚本目录被修改；请保留备份并从新版本修复"。装完 state 文件写 `{owner, configPath, target, version, scaffold, backupDir}`，`state()` 会校验 `owner` 与 `configPath`，防止跨配置文件误操作。

### 5.6 退出码含义（`island-hook.cjs`）
- **所有路径都是 `process.exit(0)`**，共三处：stdin 不是 JSON / 无 `hook_event_name`；主流程异常；正常结束。这是显式的 fail-open 契约（文件头注释第 6–8 行）：岛不可用、超时、任何错误都退出 0 且不写 stdout，Codex 回落到自己的原生审批 UI。
- 输出（仅 PermissionRequest 且拿到 delivered 决定时写 stdout）：
  - `allow` → `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}`
  - `deny` → 同上但 `{"behavior":"deny","message":"已在 Island HUD 中拒绝此请求"}`
  - `return` 与非 `delivered` 状态 → **不写 stdout**（等价交回原生 UI）。
- 跳过不询问：`permission_mode === 'bypassPermissions'`，或工具名命中 `READ_ONLY` 正则（`Read|Glob|Grep|TodoWrite|TodoRead|WebFetch|WebSearch|TaskOutput|TaskStop`）→ 直接返回，不发请求。
- 输入上限：`MAX_STDIN_CHARS = 1 << 20`（1MiB），超限把已读内容清空并 fail-open；另有 10 秒读 stdin 兜底超时（`.unref()`）。
- 平台限制（注释明说）：Codex 只在交互式 TUI 触发 `PermissionRequest`；`exec` 模式强制 `approval_policy=never`，审批根本到不了 hook。

### 5.7 DSH 侧的生命周期注意点（`integrations/dsh/index.mjs`）
- 事件上报串行化：`report()` 用 `reports` Map 按 `sessionId` 串成 Promise 链，注释说明是为了防止并发的独立 socket 乱序导致"迟到的 tool-start 覆盖 native turn-end"。
- 心跳：`PluginHeartbeat` 每 15000ms 一次，`.unref()` 不阻塞退出。
- 诊断日志只在 `config.diagnosticsPath` 配置时写 NDJSON，且 `appendFileSync` 失败被吞（"诊断失败不得改变请求处理"）；日志字段只有 `event/status/reason`，不含 payload 正文。

---

## 6. 对 daozai 的迁移要点

### 6.1 daozai 现状（`integrations/agent-notices/bridge/channel.cjs`）
- 传输：Windows `\\.\pipe\daozai-<48hex>`，非 win32 用 `tmpdir/daozai-<48hex>.sock`；descriptor `bridge/live.json`，写入用 `{mode:0o600}` + tmp+rename（`serve()` → `publish()`）。
- 认证：**HMAC-SHA256 双向 proof**。客户端发 `{key, provider, kind, proof: mac(secret,'client',key,provider,kind)}`；服务端回 `{content, proof: mac(secret,'server',key,content)}`。`secret` 与 `instance` 都是 `randomBytes(24).toString('hex')`，secret 不出现在任何线上帧里（`exchange()` 与 `serve()` 的 `frame` 回调）。
- 帧上限：`limit = 192 * 1024`（192KiB），行超限或缓冲超限即 `destroy()`（`frame()`）。
- 握手：服务端 `socket.setTimeout(1500)` 作为握手/首帧超时，ready 与已投递后 `setTimeout(0)` 取消；`server.maxConnections = 40`；`open.size >= 32` 时新请求被 `destroy()`。
- 超时：客户端握手 1200ms，`finished` 2000ms，`provider === 'codex'` 6 小时（硬编码 if）。
- kind：`ready`/`finished`/`approval`/`question`；provider 只允许 `dsh`/`codex`（`serve()` 内白名单）。
- 策略门控：descriptor 带 `policy{<provider>:{interactive, completion}, content}`，`accepts()` 按 kind 判 `interactive`/`completion`；`display` 侧 `enabled()`/`wantsText()` 读同一 policy。
- 在线判定：`mounted()` 维持一条持久 `ready` socket，只有它才算 `dshReady`；最后一个 ready socket 断开时输出 `{dshReady:false}`。
- 隐私：文件头注释"descriptor 只含本地发现信息，绝不含请求或回复内容"；帧只解析不落盘。

### 6.2 必须保留（安全/隐私，不能倒退）
1. **HMAC 双向握手**。island-hud 的 token 明文在 descriptor 中并随每一帧发送（`hub.js` → `handleSocket()` 比对 `msg.token`），任何读到 descriptor 的同用户进程都能伪造请求；daozai 的 proof 模式不传输 secret，还额外证明"对面确实是真服务端"。移植时不得为了简单退回静态 token。
2. **descriptor 收紧权限 + 路径指纹校验**。daozai 现在靠 `0o600` + `endpoint` 前缀/48hex 正则（`discover()`）；island-hud 额外用 `icacls` 关继承、只留当前用户/SYSTEM/Administrators（`hardenDescriptor()`）。建议 daozai 在 Windows 上补这一层（不替换 HMAC，只是加一层）。
3. **一连接一请求 + 队列上限**。daozai 的 `open.size>=32` / `maxConnections=40` 必须保留；island-hud 的 `pending` 没有显式条数上限，只有 `closed` 1000 条去重表，直接照搬会让岛的内存被刷爆。
4. **192KiB 帧上限**。island-hud 是 512KiB；daozai 更保守，保持现状即可，但注意要在迁移 approval 详情（`tool.operation` 等）后复核：DSH 的 `take()` 每个字段截 16000 字符，多字段叠加可能接近上限。
5. **正文不落盘 / 日志只记元数据**。island-hud 的 `log()` 只记 `provider/kind/id/session` 与终态 `status`（`hub.js` 各调用点），从不打印 payload——这条可以直接照抄到 daozai 的 `emit` 日志里。

### 6.3 可以借鉴（island-hud 更成熟的地方）
1. **receipt 终态回执**（`hub.js` → `respond()` + `client.js`）：决定投递后由客户端回 `delivered/already_handled/expired`，2 秒超时记 `adapter-receipt-timeout`。daozai 目前只有 `finished` 有 `{"done":true}`（`channel.cjs` `serve()`），approval/question 没有投递确认，岛无法区分"点下去生效了"和"对面已经没了"。建议给 approval/question 加终态帧。
2. **超时分档表驱动**（`contract.js` → `TIMEOUT_MS`/`timeoutFor()`）：daozai 现在是 `channel.cjs` 里的三元硬编码（`kind==='finished' ? 2000 : 6h`），改成 provider×kind 表后新增 Claude/ZCode 不需要改分支。
3. **失败分类透传**（`client.js` → `exchange()` 的 `catch`）：把 `already_handled`/`expired` 与真正的 `failed` 区分开并回给服务端。daozai 现在校验失败一律 `destroy()`，排障时看不出是协议错、重放还是超时。
4. **重复投递抑制表**（`hub.js` → `finish()` 的 `closed` 环形 1000 条）：断线重连、宿主重试场景下避免同一请求弹两次卡片。
5. **`at` 时间戳防伪**（`validateRequest()`：只接受 `>0 && <= now+1000`）：daozai 的 `finished` 通知若带时间戳，可照此夹取，避免宿主时钟/伪造导致通知排序错乱。
6. **question/answer 校验规则可整段移植**：1–20 题、单题 20000 字符、每题 50 选项、label 唯一、非多选时"选项与自定义互斥"、`selected` 空数组=跳过（`questionsOf()`/`answersOf()`）。daozai 侧目前没有多轮问答的 wire 级校验，这是移植多轮问答的前置条件。
7. **`surface` 字段**（≤120 字符，超长静默丢弃）：可用来区分同一 provider 的不同界面/来源，daozai 目前没有对应概念，属于低风险增量。

### 6.4 不建议照搬
- 明文 token 认证（理由见 6.2-1）。
- 内部对外 loopback TCP：island-hud 只为把管道转发给 C# helper 才在 `127.0.0.1:0` 起端口；daozai 的约束是"不开放网络 TCP 端口"，应继续只用命名管道（daozai 的 `channel.cjs` 在 win32 走管道、非 win32 走 unix socket，已满足）。
- 512KiB 帧上限与 500 字符 id：daozai 的 192KiB 更保守，无需放宽。
- approval/event payload 完全不校验：island-hud 只靠帧上限兜底，daozai 移植 approval 详情时应补字段级校验（`tool.name` 长度、`operation/target` 截断），否则渲染层要面对任意长度输入。

---

## 附：关键常量速查

| 项 | island-hud | daozai 现状 |
|---|---|---|
| 传输 | `\\.\pipe\island-hud-<pid>-<8hex>`（内部 loopback TCP 仅供 helper） | `\\.\pipe\daozai-<48hex>` |
| 认证 | descriptor 内明文 token（48hex），每帧携带 | HMAC-SHA256 双向 proof，secret 不上线 |
| descriptor | `%LOCALAPPDATA%\IslandHUD\integration-v2.json`，`{v,pipe,token,pid,createdAt}` | `bridge/live.json`，`{endpoint,secret,policy}`，mode 0600 |
| 帧上限 | 512KiB | 192KiB |
| 握手超时 | 2000ms（服务端） / 1500ms（客户端连接） | 1500ms（服务端 socket）/ 1200ms（客户端总） |
| 决定回执 | 有，2000ms 超时 | 仅 `finished` 有 `{done:true}` |
| provider | dsh/zcode/codex/claude/chatgpt | dsh/codex |
| kind | approval/question/event | ready/finished/approval/question |
| approval 超时 | dsh 2min / codex 6h / 其余 24h | 6h（硬编码，仅 codex 分支） |
| 队列上限 | pending 无上限，closed 1000 去重 | open 32 / 连接 40 |
