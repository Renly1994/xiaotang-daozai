# island-hud 会话跳转与正文读取层调研

调研对象：`D:\Codex\island-hud`（Electron）。方式：只读源码，未修改任何文件。
范围：任务指定的 9 个文件（`completion-reader.cjs`、`jsonl-tail.cjs`、`session-store.cjs`、`session-index.cjs`、`transcript-discovery.cjs`、`jump-target.cjs`、`dsh-navigation.cjs`、`usage-store.cjs`、`archive-store.cjs`）。
约定：每条结论标注 `文件 → 函数/常量`，可直接定位。仅在本层无法得出的事实，会明确标注"由调用方注入"。

---

## 1. 完成正文读取（CompletionReader + JsonlTail）

### 1.1 读哪个文件

- 读取入口是 `completion-reader.cjs → CompletionReader.read({ file, sessionKey, turn })`，`file` 由调用方传入，本文件不做路径推导。
- 文件来源链路在 `transcript-discovery.cjs`：
  - `listJsonlFiles(root, { maxFiles = 500, maxDepth = 8 })` 递归收集 `*.jsonl`（`maxFiles` 上限 500，深度上限 8）。
  - `discoverTranscripts(root, { provider, surface })` 对每个文件调 `discoverTranscriptFile()`。
  - `normalizeRecord()` 把记录归一成会话条目，并把 `sourceFile`（`path.resolve` 绝对路径）与 `sourceOffset`（该行字节偏移）写进条目。
  - 两个便捷封装：`discoverCodexTranscripts()`（surface 默认 `codex-cli`）、`discoverClaudeTranscripts()`（surface 默认 `claude-cli`）。
- DSH 侧的 transcript 是 **zstd 压缩的 JSONL**：`CompletionReader.scan()` 构造 `JsonlTail` 时固定传 `compressed: true`。若把未压缩 JSONL 交给它，`JsonlDecoder.push()` 会因 `frameLength()` 抛 `Invalid zstd frame header` 置 `decoder.invalid = true`，最终返回 `unavailable`。

### 1.2 如何定位某一轮

- `CompletionReader.scan(file, turn)` 用事件类型推进：
  - `turn/start`：`current = d.turn || current + 1`，置 `readingTurn = (current === turn)`，清空 `candidate`。
  - `assistant/message` 且 `current === turn`：`candidate = textOf(d.message?.content || d.content)`。
  - `turn/end` 且 `readingTurn`：结算 `status:'ok'`。
- 正文抽取 `textOf(content)`：字符串直接返回；数组仅保留 `type === 'text'` 的块并以 `\n` 连接；其他类型返回空串。
- 结论：**轮次只由 transcript 内的 `turn/start`/`turn/end` 计数决定**，不依赖时间戳、不依赖 `turn` 字段本身（字段缺失时退化为自增计数）。

### 1.3 容量上限

| 项目 | 值 | 位置 |
|---|---|---|
| 正文上限 | 1 MiB（按字节截断） | `completion-reader.cjs:5` `LIMIT = 1024 * 1024` |
| 缓存总字节上限 | 2 MiB | `CompletionReader.read` 淘汰条件 `this.bytes > 2 * LIMIT` |
| 缓存条目上限 | 4 条 | `CompletionReader.read` `while (this.cache.size > 4 ...)` |
| 单行上限 | 8 MiB | `CompletionReader.scan` `maxLineBytes: 8 * LIMIT` |
| 单次读取切片 | 64 KiB | `CompletionReader.scan` `maxReadBytes: 64 * 1024` |
| 解码器缓冲上限 | 16 MiB | `jsonl-tail.cjs:6` `LIMIT` |
| 发现阶段单行上限 | 512 KiB | `transcript-discovery.cjs:6` `MAX_LINE_BYTES` |
| 发现阶段单次读取 | 1 MiB | `transcript-discovery.cjs:7` `DEFAULT_READ_BYTES` |

- 截断语义：`truncated: buffer.length > LIMIT`，正文取 `new StringDecoder('utf8').write(buffer.subarray(0, LIMIT))`，即按字节切但用 `StringDecoder` 保证不切碎 UTF-8 多字节字符（不会出现半个汉字）。

### 1.4 缓存策略

`CompletionReader` 三个字段：`cache`（Map）、`inflight`（Map）、`bytes`（累计字节）、`epoch`（代际计数）。

- **缓存键**：`JSON.stringify([sessionKey, turn])`（`read()`）。
- **失效依据**：文件身份 `identity = [dev, ino, birthtimeMs, size, mtimeMs].join(':')`（`read()`）。identity 相同即命中，命中时执行 `delete` + `set` 实现 **LRU 触碰**。
- **请求合并**：`inflight` 以 identity 去重，同一 (sessionKey, turn, identity) 并发只扫一次（`read()` 第 21 行）。
- **代际作废**：`clear()` 执行 `epoch++`；扫描完成后若 `epoch` 已变，返回 `{ ...value, status: 'expired' }` 且**不写缓存**。用于插件卸载/重连时让在途结果失效。
- **按会话回收**：`retain(sessionKeys)` 遍历缓存，键里 `JSON.parse(key)[0]` 不在集合内即扣除字节并删除（会话被移除后立即释放正文内存）。
- **膨胀控制**：`while (this.cache.size > 4 || this.bytes > 2 * LIMIT)` 从头淘汰。

### 1.5 状态值与失败返回

`read()` 只在四种状态间流转：

- `ok`：命中 `turn/end`。
- `unavailable`：`fs.promises.stat` 失败（文件不存在/无权限）、`tail.identity` 为空、`decoder.invalid` 为真、或从未命中 `turn/end`。
- `expired`：扫描期间 `clear()` 发生。

三种状态都返回同构对象 `{ status, turn, text, truncated }`，失败时 `text: ''`、`truncated: false`，**不抛异常**。

### 1.6 性能优化手段

1. **分片读**：`JsonlTail.poll(maxBytes)` 每次只从当前 offset 读 `maxReadBytes`（此处 64 KiB），避免一次性读入大文件。
2. **setImmediate 让出**：`scan()` 每轮 `poll()` 后 `await new Promise(resolve => setImmediate(resolve))`，注释明确目的是"a large transcript cannot freeze the UI"（Electron 主进程与渲染共用事件循环）。
3. **增量解码**：`JsonlTail` 维护 `offset`，只在文件被替换/截断/重写时 `reset()`，正常路径增量推进。
4. **zstd 帧级解码**：`jsonl-tail.cjs → JsonlDecoder.push()` 按完整 zstd 帧解码（非流式逐块），`frameLength()` 负责切帧边界。
5. **防重放扫描**：identity 命中即返回缓存，避免重复全量扫描。

### 1.7 值得注意的实现代价

- `scan()` 构造 `JsonlTail` **未传 `startOffset`**，即每次缓存未命中都从文件 offset 0 重放整个 transcript。`jsonl-tail.cjs` 注释说明 `startOffset` 本是"大 transcript 从当前状态续读"的优化点，这里没有使用 → 首次读取长会话是 O(文件大小)。
- 判断顺序有一处细节：`if (tail.caughtUp) break;` 先于 `if (!tail.identity || tail.decoder.invalid) return unavailable;`。两条件同时为真时会先 break，最终由结尾 `return value || { status: 'unavailable' }` 决定——已结算的 `ok` 会保留，未结算则 `unavailable`。移植时若要求"解码异常必须判失败"，需把 invalid 判断提到 caughtUp 之前。

---

## 2. 隐私边界

- **读什么**：仅读 `assistant/message` 的 `content` 文本块（`textOf`）。用户输入、工具参数、工具结果均不进入完成正文缓存。
- **是否落盘**：`completion-reader.cjs` 全文没有任何 `fs.write*`；`JsonlTail` 只有 `fs.openSync/readSync/fstatSync`。**正文只存在于内存 Map**。
- **缓存保留多久**：无 TTL，由三件事回收——4 条/2 MiB 的 LRU 淘汰、`clear()` 代际作废、`retain()` 按会话回收。进程退出即消失。
- **其它存储不含正文**（跨文件验证）：
  - `session-index.cjs → normalizeEntry()` 字段白名单里没有正文字段，`lastSummary` 走 `text()` 截断到 `MAX_TEXT = 500`。
  - `archive-store.cjs → add()` 只存 `title`/`project`（各 `slice(0, 500)`）。
  - `usage-store.cjs` 只存计数器与毫秒数。
- **失败时返回**：统一返回空串 + `unavailable`/`expired`，**不回传错误详情、不回传路径**，避免把文件系统结构泄漏给渲染层。
- **副作用面**：本层唯一的磁盘写是 `dsh-navigation.cjs → writeDescriptor()`（写 token，见第 5 节），与正文无关。

---

## 3. 会话索引与存储

### 3.1 标识与主键

- 主键 `keyOf(provider, rawId) = JSON.stringify([provider, rawId])`。`session-index.cjs`、`archive-store.cjs` 各自定义了同格式的 `keyOf`；`session-store.cjs` 从 `integrations/contract` 导入。
- `rawId` 是 provider 原生会话 ID（`session-index.cjs → normalizeEntry` 兼容入参 `rawId || sessionId`，上限 500 字符）。`sessionId` 字段归一后与 `rawId` 同值。
- **provider 集合有两套**：`session-store.cjs → entry()` 用 `PROVIDERS.has(provider)`（来自 `integrations/contract`）；`usage-store.cjs:7` 与 `archive-store.cjs:6` 各自硬编码 5 个：`dsh`、`zcode`、`codex`、`claude`、`chatgpt`。移植时必须统一为单一枚举，否则会出现"能建会话但统计不到"。
- `cwd`/`pid`：`session-store` 只从事件 `payload.cwd` 取 `cwd` 与 `project = path.basename(cwd)`；`snapshot()` 输出的 `jumpTarget` 只有 `{ provider, sessionId: rawId }`，**不含 pid**。`pid` 只在 `jump-target.cjs` 里被读取，由调用方注入的进程列表带来。

### 3.2 运行时状态（session-store.cjs）

- 内存三个 Map：`sessions`（会话表）、`requests`（待处理请求）、`requestsBySession`（按会话分组）、`unassociatedRequests`（无会话归属）、`confirmations`（已决结果回执）。
- `staleMs` 默认 `30 * 60000`（30 分钟）；`now` 可注入以便测试。
- `applyEvent(envelope)` 白名单事件：`UserPromptSubmit`、`PreToolUse`、`PostToolUse`、`SessionStart`、`Stop`、`Interrupt`、`SessionEnd`、`SessionTitle`、`ApprovalDecided`、`QuestionAnswered`，其它事件 `return false`。
- 关键身份规则写在代码注释里：`if (!envelope.sessionId) return false; // Never infer identity from cwd or recency.` —— **禁止用 cwd 或时间邻近性猜会话**，这条必须原样保留到 daozai。
- 阶段推导：`UserPromptSubmit`/`PreToolUse`/（非 zcode 的）`SessionStart` → `running`；`Stop` → `completed` + `completedAt`；`Interrupt`/`SessionEnd` → `idle`。`zcode` 的 `SessionStart` 只登记不置 `running`（注释：桌面端冷开旧会话也会触发）。
- `UserPromptSubmit` 保留 `lastUserText = payload.prompt.slice(0, 300)`；`SessionTitle` 保留 `title`。
- `snapshot()` 排序用 `rank(phase)`：`waitingForApproval(0) < waitingForAnswer(1) < running(2) < completed(3) < idle(4)`，同阶段按 `updatedAt` 降序。
- 降级与提醒：无阻塞请求且无当前工具时，`now - updatedAt > (phase==='completed' ? 5 : 3) * 60000` 强制降为 `idle`；`needsAttention = currentTool.since > 30000` 且无审批/问答。
- 过期回收：非历史条目、无待处理请求、`now - updatedAt > staleMs` 时从 `sessions` 删除；`confirmations` 同样按 `staleMs` 清理。

### 3.3 持久化（session-index.cjs）

- 构造要求**绝对路径**：`if (!path.isAbsolute(file)) throw new Error('Absolute session index path required')`。
- 格式：单文件 JSON `{ version: 1, entries: [...] }`；`VERSION = 1`。
- 保留策略：`MAX_ENTRIES = 500`、`TTL_MS = 30 天`、`MAX_TEXT = 500`。
- `prune()` 有两条规则：① 仅删除 `phase === 'completed'` 且 `completedAt < now - ttl` 的条目；② 按 `lastSeen` 降序保留前 `maxEntries` 条。**`running`/`idle` 条目不会被 TTL 清理**，只受 500 条上限约束。
- 写入安全：`save()` 生成 `file + '.' + randomUUID() + '.tmp'`，`writeFileSync({ flag: 'wx', mode: 0o600 })` 后 `renameSync` 原子替换；失败时 `log('session index save failed: ...')` 并清理临时文件。
- 读取容错：`load()` 校验 `version` 与 `entries` 数组，非 ENOENT 错误只记日志、不清空（对畸形 JSON 会保留空表继续运行）。
- `normalizeEntry()` 会剥除 C0 控制字符（`replace(/[\u0000-\u0008...]/g, '')`）并 `trim()`，最后用 `filter` 丢掉 `undefined` 与空串字段。

### 3.4 归档/隐藏（archive-store.cjs）

- 语义是**纯本地隐藏列表**，注释明确："Never writes to DSH session files or profiles."。
- `PROVIDERS` 5 个；`KINDS = {'archived','deleted'}`（`deleted` 也只是本地隐藏，不删磁盘文件）；`VERSION = 2`（兼容读 v1，v1 统一按 `archived`）。
- 上限：`MAX_ENTRIES = 500`、`TTL_MS = 30 天`、`LIST_LIMIT = 200`。
- 状态门禁：`canHide(session)` 要求 `phase ∈ {idle, completed}` 且无待处理请求；`shouldHide(session, hidden)` 在会话重新变活跃（`running`/`waitingFor*`）或有待处理请求时**自动取消隐藏**。
- 写入：`persist()` 同样 `randomUUID` 临时文件 + `wx` + `mode 0o600` + `rename`；`add()`/`remove()` 写盘失败会回滚内存状态并返回 `{ ok: false, reason: 'storage' }`。
- 损坏自愈：`load()` 解析失败时把原文件改名为 `file + '.bad-' + now` 备份，清空列表继续运行，并记日志。

---

## 4. 跳转解析（jump-target.cjs）

`resolveJumpTarget({ session, processes, terminals, clients, stat })` 的决策顺序是**短路**的，依次为：

| 序 | 条件 | 返回 kind | confidence | fallback |
|---|---|---|---|---|
| 1 | `!session` 或非对象 | `unsupported` | `none` | — |
| 2 | `session.provider === 'codex'` | `deep-link` | `exact` | `workspace` |
| 3 | 在 `processes` 中命中 | `process` | 命中 `sessionId`/`rawId` 为 `exact`，命中 `pid` 或 `cwd` 为 `high` | `workspace` |
| 4 | 在 `clients` 中命中（同 provider 且 `running !== false`） | `client` | `client` | `workspace` |
| 5 | 在 `terminals` 中按 cwd 命中 | `terminal` | `workspace` | `workspace` |
| 6 | `cwd` 存在 | `workspace` | `workspace` | `none` |
| 7 | 其余 | `unsupported` | `none` | `none` |

- Codex 深链格式（`codexTarget()`）：`codex://threads/<encodeURIComponent(rawId || sessionId)>`，`surface` 默认 `codex-desktop`，id 走 `text()` 上限 128。**Codex 完全绕开进程/客户端探测**，不校验会话是否仍存在。
- 进程匹配是三个 OR 分支（`resolveJumpTarget` 第 24–27 行）：会话 ID 相等 → `pid` 相等 → `cwd` 相等。因此"cwd 相同"会把会话错误归到同项目下另一个进程，这正是它降级为 `high` 而不是 `exact` 的原因。
- 路径比较 `samePath(a, b)`：`path.normalize()` 后 `toLowerCase()` 全量小写比较。该实现无平台分支，Windows 正确；若将来跨平台需补 `caseSensitive` 判定。
- cwd 必须经 `existingDirectory(cwd, stat)` 用注入的 `stat()` 确认存在，否则视为空 → 直接掉到 `unsupported`。这条避免把已删除目录当成可跳转目标。
- 失败原因字符串共三种：`'会话不存在'`、`'Codex 会话标识无效'`、`'未找到可跳转的客户端、terminal 或项目目录'`。
- 依赖注入说明：`processes`、`terminals`、`clients`、`stat` 全部由调用方提供，本文件不探测系统状态；`session-store.cjs → snapshot()` 只提供 `{ provider, sessionId }`，`pid`/`cwd` 需要调用方补齐。

---

## 5. DSH 导航（dsh-navigation.cjs）

### 5.1 为什么是 HTTP localhost

类注释（第 31–36 行）写明用途："Loopback queue used by the DSH browser half to open an exact Session ID." 消费方是 **DSH 的浏览器半（页面侧插件）**，页面里没有 `node:net`，唯一可用的本机传输是 `fetch()`，所以选了 loopback HTTP。配套的 `cors()` 直接 `Access-Control-Allow-Origin: '*'` 并在 `handle()` 里应答 `OPTIONS`，正是因为页面 origin 与 `127.0.0.1:port` 不同源。

### 5.2 端口与 token

- `DEFAULT_PORT = 47831`；优先级：构造参数 `port` > descriptor 里保存的 `port` > 环境变量 `ISLAND_HUD_NAV_PORT` > 默认值（构造函数）。
- `start()` 监听 `127.0.0.1`（`server.listen(port, '127.0.0.1')`，**不绑定 0.0.0.0**）；遇 `EADDRINUSE` 且原端口非 0 时改监听 `0` 取临时端口。
- token：`randomBytes(TOKEN_HEX / 2).toString('hex')`，`TOKEN_HEX = 48` → 24 字节 = **48 位十六进制（192 bit）**；优先复用 descriptor 里已保存的 token（跨重启保持插件可用）。
- 描述文件：`writeDescriptor()` 写 `{ v: 1, port, token, path }`，`v !== 1` 或 token 不匹配 `/^[a-f0-9]{48}$/` 或 port 非法时 `readDescriptor()` 返回 `null`（视为未配置）。写入用 `randomUUID` 临时文件 + `flag: 'wx'` + `mode: 0o600` + `rename`。

### 5.3 协议（长轮询队列）

- 路径常量 `NAV_PATH = '/island-hud/navigate'`；`authorize(url)` 只校验两件事：`pathname === NAV_PATH` 且 `searchParams.get('token') === this.token`，失败统一 404 `not_found`。**token 是唯一身份凭据，没有 pid/SID/进程校验。**
- 应用侧 `request(sessionId, timeoutMs = 2500)`：`sessionId` 非字符串/空/长度 > 500 直接 `{ status: 'expired', sessionId: null }`；否则生成 `randomUUID()` 的 `requestId` 入 `queue`，超时 resolve `{ status: 'unavailable' }`；`queue.length > 8` 时从头丢弃。
- 页面侧 `GET`：对 `queue.shift()` 出队；空队列返回 **204**；有请求则移入 `inflight` 并回 `{ protocol: 1, requestId, sessionId }`。
- 页面侧 `POST`：请求体累计 > 4096 字节即 `request.destroy()`（`handle()` 的 `data` 处理）；JSON 必须含字符串 `requestId` 且 `status ∈ {navigated, expired, failed}`，否则 400 `bad_ack`；`settle()` 命中返回 200 `{ settled: true }`，未命中返回 410。
- 生命周期：`stop()` 把所有等待者以 `failed` 结算、清空队列与 inflight、关闭 server。

### 5.4 安全约束（移植时必须逐条评估）

1. **它确实开了一个监听 TCP 端口**（即使只绑 127.0.0.1），这正是 daozai 明令禁止的部分。
2. token 放在 **URL query** 里，会被浏览器历史/代理日志记录；安全依赖"仅本机 + 192 bit 随机 + 0600 descriptor"。
3. CORS 为 `*`：任何能拿到 token 的页面都可 `GET` 窃取下一个 `sessionId`，或用已知 `requestId` 伪造 `navigated` 结算。
4. `authorize()` 用 `===` 比较 token，非常量时间（理论上可测时，实际被网络抖动掩盖）。
5. token 以明文存于 descriptor（`mode 0o600`，同用户可读）。
6. 无频率限制：GET 空转无成本上限，靠 `setTimeout(2500)` 与 `queue <= 8` 间接约束。

---

## 6. usage 与 archive 统计

### 6.1 usage-store.cjs

- 结构 `{ version: 2, since, totals, providers, surfaces, metrics, windows }`；`load()` 兼容 `version ∈ {1, 2}`。
- `providers` 固定 5 个（`dsh`/`zcode`/`codex`/`claude`/`chatgpt`）；每个 provider 与每个 `surface` 计数为 `{ events, approvals, questions, completed }`。
- 全量 `metrics` 为 8 个字段：`events`、`approvals`、`questions`、`completed`、`failed`、`expired`、`waitMs`、`durationMs`（`emptyMetric()`）。
- 三个滚动窗口 `windows.daily/weekly/monthly`，起点用 `utcStart(value, window)`：日 = UTC 当日 00:00，周 = UTC 周一 00:00（`getUTCDay() || 7`），月 = UTC 月首 00:00。`load()` 时若窗口 `since` 与当前周期不符就丢弃旧窗口；`#refreshWindows()` 在每次写指标前检查并重置。
- 聚合有两条路径：
  - `recordEvent(provider, event, surface, meta)`：累加 `totals` 与 `providers[provider]` 与 `surfaces[surface]`，并 `#touchMetric()` 写全量指标 + 三个窗口；`ApprovalDecided` 记 approvals，`QuestionAnswered` 记 questions，`Stop`/`SessionEnd` 记 completed，`Failure`/`Failed` 记 failed，`Expired` 记 expired。
  - `recordRequest(provider, requestId, kind, surface, meta)`：用 `provider + ':' + requestId` 做去重，`seenRequests` 上限 2048（FIFO 淘汰），只写 approvals/questions 与 `waitMs`。
- 注意不对称（移植时需修正而非照抄）：`recordOutcome()` 与 `recordEvent()` 里的 `failed`/`expired` **只写 metric，不写 `totals`/`providers`/`surfaces`**；即"失败"在总表里不可见，只在 `metrics` 与窗口里可见。
- 持久化：构造要求绝对路径；`save()` 每次记录都同步重写整个 JSON（`flag: 'w'`, `mode 0o600`，`tmp` + `rename`）→ **每事件一次全量落盘**，daozai 应改为防抖批量写。
- `reset()` 清空并立即落盘。

### 6.2 archive-store.cjs

见 3.4。统计相关补充：`count(kind)`、`keys(kind)`、`list(kind, limit = 200)`（按 `hiddenAt` 降序），`trim()` 用 O(n) 找最旧条目淘汰到 500 条以内。

---

## 7. 对 daozai 的迁移要点

### 7.1 可直接复刻的行为

1. **完成正文"仅内存"边界**：照搬 `CompletionReader` 的三状态（`ok`/`unavailable`/`expired`）+ 4 条/2 MiB/1 MiB 三档上限 + identity 失效 + inflight 合并。这是本层最有价值的契约。
2. **会话主键与身份铁律**：`keyOf(provider, rawId)` + "禁止用 cwd/时间最近推断会话"（`session-store.cjs → applyEvent` 注释）。daozai 侧新增任何会话都必须带 provider + 原生 sessionId。
3. **跳转置信度分级**：`exact`（原生会话 ID / Codex 深链）> `high`（pid / cwd 进程匹配）> `client` > `workspace`，UI 上应按置信度决定是"直达"还是"仅打开项目"。
4. **原子落盘**：`randomUUID` 临时文件 + `flag:'wx'` + `mode 0o600` + `rename`（`session-index.save()`、`archive-store.persist()`、`usage-store.save()`、`writeDescriptor()`），daozai 已有同类需求，务必沿用而非 `write + fsync`。
5. **状态门禁**：`canHide`/`shouldHide` 的"活跃即自动取消隐藏"语义。

### 7.2 需要改造或不应照搬

1. **不要实现 `DshNavigationServer`**：它是本层唯一开监听 TCP 的组件，与 daozai 安全约束直接冲突（第 5.4 节）。
2. **不要把正文写进索引/归档**：island-hud 本身没有犯这个错，移植时也保持 `lastSummary ≤ 500`、`title/project ≤ 500`。
3. **`recordOutcome`/失败计数的统计缺口**（6.1）需补齐到 `totals`/`providers`。
4. **`usage-store` 每事件同步全量落盘**需改为内存累加 + 定时/退出时落盘。
5. **`scan()` 未用 `startOffset`** 导致全量重放：daozai 若直接读 DSH transcript，应按 `sourceOffset` 续读，或用"仅解析尾部 N MiB"上限。
6. **`session.pid` 断裂**：`session-store.snapshot()` 不产出 `pid`/`cwd`，而 `jump-target` 依赖它们；移植时必须明确由谁填充（daozai 可从 DSH 进程表/端口归属反查），否则 `process` 档永远不命中。
7. 本层未覆盖、需上游任务补齐的事实：DSH transcript 根目录、`live.json` 之外的 descriptor 路径、页面侧插件如何消费导航请求（这些不在这 9 个文件内）。

### 7.3 TCP 端口禁止约束下的导航替代方案

**根因分析**：island-hud 用 HTTP loopback 不是因为需要网络，而是因为"Electron 主进程 ↔ 页面半"之间只有 HTTP 可用。daozai 的对应链路是"Rust 进程 ↔(stdio)↔ bridge worker ↔(命名管道)↔ DSH/Codex 插件"，这条链路**已经有比 island-hud 更强且更省的认证通道**（`integrations/agent-notices/bridge/channel.cjs`）：

- 命名管道 `\\.\pipe\daozai-<48hex>`，实例随机；descriptor `live.json` = `{ endpoint, secret, policy }`，`mode 0o600`，且**只含本地发现信息，不含正文**（该文件第 2 行注释）。
- 双向 HMAC 握手：`mac(secret, 'client'|'server', ...)`（`createHmac('sha256')`），48 位十六进制的 `nonce()` 作 key；`serve()` 对 provider/kind/proof 三重校验后才建会话。
- NDJSON 分帧 + `limit = 192 * 1024` 单帧上限（`frame()` 超限直接 `socket.destroy()`）；握手前 socket 超时 1500ms，`exchange()` 整体 1200ms。
- 结论：**daozai 已有"端口 + token"方案的安全等价物，且是双向认证的；引入 HTTP 只会扩大攻击面。**

按优先级给出四档方案：

**方案 A（首选，零新增端口）：复用 daozai 现有「stdio + 命名管道」双层通道**

- 真实拓扑（读码确认，不能按 island-hud 类比）：daozai 是 worker 的**父进程**——`src/agent_cards.rs → Service::start()` 将 `config_dir()/agent-notices/bridge/channel.cjs` 拉起为子进程，`stdin`/`stdout` 均 piped，并校验磁盘脚本与 `include_bytes!` 内嵌版本逐字节一致（篡改即拒绝启动）；**管道服务端在 worker 的 `serve()` 里**，DSH/Codex 侧插件通过 `channel.exchange()` 作为管道客户端接入。即 daozai 与插件之间是「stdio → worker → 管道」两跳，navigate 命令可走这条已认证链路。
- 最小改动三处（都在 `integrations/agent-notices/bridge/`，不动 daozai 对外接口）：
  1. `channel.cjs → frame(process.stdin, ...)` 增加命令分支，识别 `{ command:'navigate', provider, sessionId, turn }` 存入有界内存队列（上限建议 8，对齐 island-hud 的 `queue.length > 8` 丢弃策略）；当前 stdin 只认 `{policy}` 与 `{id, reply}` 两种帧。
  2. `channel.cjs → accepts(policy, provider, kind)` 增加 `kind === 'navigate'` 并在 `serve()` 的 hello 放行；由插件用 `exchange('dsh','navigate', ...)` **拉取**该命令（`exchange()` 本身是请求-响应式，天然适合轮询拉取）。
  3. `bridge/index.mjs → apply(ctx)` 把拉到的命令转交 DSH 页面侧插件完成"打开指定 sessionId"。
- 硬约束（读源码得出）：`serve()` 对 `kind === 'ready'` 的连接在收到第二帧即 `socket.destroy()`（presence socket 一次性语义），故 navigate **必须用独立 kind 的短连接**，不能挂 ready 长连接；`exchange()` 默认 1200ms 超时，仅 `finished`（2000ms）与 `codex`（6 小时）有特例，navigate 需单独设超时；并发受 `server.maxConnections = 40`、未决请求 `open.size >= 32` 约束，属"尽力而为"型跳转。
- 隐私：navigate 帧只带 `sessionId`/`turn`，不带正文，不改变现有边界。
- 未决依赖：DSH 是否有 node 侧 → 页面侧的命令通道。`index.mjs` 现仅用 `ctx.on` 事件与 `ctx.get` 服务，未出现面向页面的推送 API；若无，则须由页面侧插件自行拉取，否则退方案 B/C。

**方案 B（不新开监听端口，仅出站）：复用 DSH 自身已监听的 127.0.0.1 端口**

- DSH Web GUI 自身已在 `127.0.0.1` 上监听（本会话为 `http://127.0.0.1:52216`），daozai 用 `ureq`（`Cargo.toml` 已依赖，含 `native-tls`）做**出站**请求，不监听任何端口，不违反约束。
- 关键纪律：必须**把目标限制为 descriptor 中记录的 `^http://127\.0\.0\.1:\d+$`**，禁止从会话正文/事件里取 URL，防止被诱导发起外连。sessionId 走 URL 编码，不用拼接。
- 相比方案 A 的代价：需要在 `live.json` 里增补 GUI origin（或读取 DSH 自己的端口发现文件），且依赖 DSH 的会话路由参数名（实现前需以实际 DSH 路由实测确认参数名，不臆测）。

**方案 C（降级但确定可行）：ShellExecute 打开深链**

- Codex 可**完整照搬** `codexTarget()` 的 `codex://threads/<encodeURIComponent(id)>`，直接 `ShellExecuteW("open", url)`；daozai 已启用 `Win32_UI_Shell`，`main.rs:976` 已在用 `ShellExecuteExW`。
- DSH 无公开深链协议，退化为"打开 DSH 界面"（`http://127.0.0.1:<port>/`），由用户自行定位会话。
- 缺点：会新开窗口/标签而非聚焦已有窗口；优点：零监听、零认证面。

**方案 D（兜底，必须实现）：窗口聚焦 + 工作区**

- daozai 已有 `SetForegroundWindow`/`ShowWindowAsync`（`desktop.rs:135-169`、`qq.rs:15-16`）与 `Cmd::Reveal` 用 `explorer.exe` 打开目录（`main.rs:753`）。
- 对应 island-hud 的 `kind: 'workspace'` / `fallback: 'workspace'`：至少在低置信度时把用户送到正确项目目录或把 DSH 窗口带到前台。注意 `desktop.rs:140` 已处理 `SetForegroundWindow` 被系统拒绝的情况（返回 `Err`），应沿用该失败路径而不是静默成功。

**决策建议**：A 为主路径（安全等价且零新增端口），C 用于 Codex（纯深链，与 island-hud 完全一致），D 作为所有失败分支的兜底；**永不实现 `DshNavigationServer`**。若 A 因 DSH 缺少 node→page 通道而受阻，则用 B 替代 A，并把 B 的出站白名单作为硬校验。

### 7.4 正文读取路径的选择

daozai 现有链路是插件在 `turn/end` 时透传正文（`bridge/index.mjs → apply` 的 `ctx.on('session/event')`，文本先 `slice(0, 32000)`），成本低、无需文件权限。是否再加"直接读 transcript"取决于需求：

- 只需实时一轮 → 继续用插件透传，不必移植 `CompletionReader`/`JsonlTail`。
- 需要回看历史轮次、或在缓存淘汰后重取 → 才需要 `CompletionReader` 能力；此时 daozai 已有 `zstd = 0.13.3`，具备解码压缩 transcript 的依赖，但仍需解决 transcript 目录发现与文件权限（`MAX_LINE_BYTES 512 KiB`、`MAX_FILES 500`、`maxDepth 8` 三个参数可直接复用）。

---

## 附：本层数字速查

- 正文上限 1 MiB｜缓存 4 条 / 2 MiB｜切片 64 KiB｜zstd 帧解码上限 16 MiB / 块 128 KiB
- 发现阶段：单行 512 KiB、单次读 1 MiB、最多 500 文件、深度 8
- 会话索引：500 条 / TTL 30 天 / 文本 500 字符；归档：500 条 / TTL 30 天 / 列表 200
- 运行时：`staleMs` 30 分钟；`needsAttention` 工具 30 秒；idle 降级 3 分钟（completed 5 分钟）
- 导航：端口 47831（可环境变量/临时端口）、token 48 hex、请求体 4096 字节、队列 8、超时 2500ms
- daozai 通道：管道名 48 hex、单帧 192 KiB、握手 1500ms、交换 1200ms、并发连接 40、未决请求 32
