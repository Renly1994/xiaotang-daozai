# 岛仔（daozai）项目交接文档

> 更新时间：2026-11-19  
> 文档用途：供新的开发 agent 接手 D:\Codex\daozai，继续维护桌面岛仔、Agent 审批/提问交互和本机测试版交付。  
> 当前交付入口：D:\Codex\daozai\target\x86_64-pc-windows-msvc\release\daozai.exe

## 0. 接手前先读

这是一个正在开发中的工作树。当前 Agent UI、bridge 和交付相关改动尚未提交，接手时先查看 git status 和 git diff，再决定修改范围。

禁止在没有确认改动归属前执行以下操作：

    git reset --hard
    git clean -fd
    git checkout -- .
    git restore .

这些操作可能删除当前未提交的 Agent 功能实现、接入脚本、测试夹具和交接文件。

项目根目录已有 AGENTS.md。它要求：

- 修复验证通过后统一覆盖桌面“岛仔－当前测试版”快捷方式指向的 release 程序。
- 交付前核对快捷方式目标和 release 文件哈希。
- 岛仔正在运行时，切换到更新后的程序并核对进程路径。
- 后续修复沿用同一入口，不另建临时交付目录。
- 全程使用中文沟通。

## 1. 项目身份与当前基线

- 项目名称：岛仔（daozai）
- GitHub 仓库：https://github.com/Renly1994/xiaotang-daozai
- 本地目录：D:\Codex\daozai
- 当前分支：main
- 当前 HEAD：08250c5
- Cargo 包版本：0.6.66
- Rust edition：2021
- 主要技术栈：Rust、eframe/egui 0.31、Windows API、Node.js bridge
- 目标平台：Windows x86_64，MSVC toolchain
- 默认二进制：daozai
- 录制器二进制：daozai-record，需要 record feature

当前已核验的 release 产物（2026-11-19 第二轮：island-hud 能力复刻后）：

- 构建产物：D:\Codex\daozai\target\x86_64-pc-windows-msvc\release\daozai.exe
- 交付文件：C:\Users\Administrator\Daozai\daozai.exe
- 文件大小：22,663,168 bytes
- SHA-256：590368A700D582172159592333916EC03817AE0C9A29C106C27D01E0FE8992ED
- 桌面快捷方式：C:\Users\Administrator\Desktop\岛仔－当前测试版.lnk
- 快捷方式目标：C:\Users\Administrator\Daozai\daozai.exe
- 最近核验时运行中的 daozai 进程 Path：C:\Users\Administrator\Daozai\daozai.exe
- 托盘登记：`Shell_NotifyIconGetRect` 返回 0x00000000，矩形落在通知区

**交付路径必须离开 D:\Codex 沙箱目录**。该目录带有 `CodexSandboxUsers` 沙箱 ACL，从此处启动的子进程会被放入沙箱作业并以中完整性（0x1000）运行；Explorer 与桌面多数用户程序为高完整性（0x3000），完整性级别不一致时 `Shell_NotifyIconW` 返回拒绝访问（错误 5），托盘图标不会出现。把同一 exe 复制到 `C:\Users\Administrator\Daozai` 后启动即恢复 0x3000，`Shell_NotifyIconGetRect` 返回 0x00000000，图标落在通知区。详见 `docs/superpowers/specs/2026-11-19-tray-icon-recovery-design.md`。

PID 会随重启变化，路径和哈希需要在每次交付前重新核验。

## 2. 产品职责与当前已实现能力

岛仔是 Windows 顶部桌面伙伴和信息岛，已有日历、天气、音乐、邮件、额度、提醒、托盘等能力。本轮交接重点是 Agent 消息提醒与交互。

当前 Agent 能力包括：

1. DeepSeek Harness（DSH）完成通知
   - 接收本轮结束事件。
   - 支持仅状态、回复摘要和本轮回复三种内容策略。
   - 支持轻提示或展开结果卡。
   - 支持 2 至 15 秒停留时间。
   - 悬停时暂停倒计时。
   - 有待处理审批/提问时，完成通知排在待处理请求之后。

2. DeepSeek Harness 审批
   - 审批请求进入待处理卡队列。
   - 可查看工具、目录、原因和完整操作。
   - 支持“仅允许这一次”“拒绝”“回客户端”。
   - 无法取得完整操作时不能在岛上允许。
   - DSH 的带有效范围授权属于原客户端语义，岛仔只展示、拒绝或交回，不伪造一次性工具审批。

3. DeepSeek Harness 提问
   - 支持单选、多选和自定义回答。
   - 多轮提问逐轮显示，一次只回答当前轮。
   - 可以返回上一轮修改答案。
   - 最后一轮统一提交所有轮次答案。
   - 选项和自定义输入互斥时会自动清理另一侧选择。
   - 可对过期后进入 continued 状态的提问进行可选延后回答；默认关闭。

4. Codex PermissionRequest
   - 通过 Codex hooks 接收当前客户端实际发出的审批。
   - 支持允许一次、拒绝和回客户端。
   - Codex 的 request_user_input 只做提示，回答仍由 Codex 原生界面完成。
   - 点击“回客户端”会交回原请求，不依赖决定退出 hook。

5. DSH/Codex 连接状态
   - 已配置：接入配置已写入，等待客户端重启或 hook 信任。
   - 已收到通知：收到了事件，但没有确认交互插件在线。
   - DSH 只有 live channel ready 成功时才显示交互插件已连接。
   - 关闭总开关或来源开关会把尚未处理的请求交回客户端。

6. 完成通知设置
   - 任务完成提醒总开关。
   - 轻提示/展开结果卡。
   - 仅状态/回复摘要/本轮回复。
   - 停留时间 2 至 15 秒。
   - 可选系统通知；系统通知不显示回复正文。

## 3. 总体架构

    main.rs
      └─ app.rs
          ├─ tray.rs / native.rs              Windows 窗口、托盘、命中测试
          ├─ model.rs                         设置、持久化、数据目录
          ├─ agent_notice.rs                  来源接入、资源导出、Node 启动
          └─ agent_cards.rs                   bridge 服务、请求队列、审批/问答 UI
                └─ integrations/agent-notices
                    ├─ connect.mjs             安装/移除 DSH 或 Codex 接入
                    └─ bridge/
                        ├─ index.mjs           DSH hook 适配和 continued watcher
                        ├─ channel.cjs         本机命名管道、NDJSON、认证
                        ├─ notify.cjs          Codex hook 入口
                        ├─ cordis.patch.yml   DSH bundle patch
                        └─ package.json

Rust 主进程通过 agent_cards.rs 启动一个隐藏窗口的 Node channel.cjs 子进程。Rust 和 Node 之间使用 stdin/stdout 的 NDJSON；Node 与客户端 bridge 之间使用本机命名管道。请求正文和回答只在内存中经过，不进入岛仔通知历史。

## 4. 关键代码文件

### Rust 主程序

- src/main.rs
  - Windows 应用入口。
  - 初始化 egui/eframe、设置、托盘和主窗口。
  - 启动后由 app.rs 驱动每帧 UI 更新。

- src/app.rs
  - 主界面、顶部岛仔、设置页和卡片表面。
  - 调用 agent_cards.rs 绘制待处理请求、详情层和完成通知。
  - 详情层尺寸需要和现有窗口高度 WIN_H 协调，避免审批原因、操作内容或问答选项溢出。

- src/agent_notice.rs
  - Provider：Dsh、Codex。
  - Status：Finished、Approval、Question。
  - wire_id/decode：保持 provider/status 的稳定编号。
  - export：把内置 docs、connect.mjs、bridge 脚本导出到 %APPDATA%\Daozai\agent-notices。
  - Setup：按开关异步安装默认 DSH/Codex 配置。
  - node_runtime：查找系统 Node.js 和 DSH 自带的兼容 Node runtime。

- src/agent_cards.rs
  - Preferences：交互开关、自动交回、完成通知、内容模式、延后提问等设置。
  - Service：启动 bridge/channel.cjs，并在 Rust 与 Node 之间收发 JSON。
  - Cards：维护待处理请求队列、完成通知、DSH ready 状态和当前悬停状态。
  - Request：保存 provider、请求类型、问题数据、答案、当前轮次、滚动位置和详情状态。
  - detail_size：按问题/原因/操作文本动态计算详情卡高度。
  - draw_details：绘制审批详情、逐轮问答、上一轮/下一轮/提交回答和自定义输入。
  - hand_back：向 bridge 发送 decision=return。
  - editing：在自定义输入或问答详情中暂停自动交回。

- src/ui_tests.rs
  - Agent 卡片尺寸、详情点击、多轮问答、连接断开释放等 UI 相关测试。
  - 当前 Agent 相关 Rust 测试已核验 8/8 通过。

- src/tray.rs
  - Windows 系统托盘菜单和退出逻辑。
  - 修改 Agent 入口或进程切换时，要确保托盘退出与主窗口退出路径仍能清理 channel 子进程。

- src/model.rs
  - 设置、原子写入、配置目录。
  - 自定义数据目录由 DAOZAI_DATA_DIR 等项目逻辑决定；默认使用 %APPDATA%\Daozai。

### Node bridge

- integrations/agent-notices/connect.mjs
  - 命令格式：
    
        node .\connect.mjs codex install
        node .\connect.mjs codex remove
        node .\connect.mjs dsh install
        node .\connect.mjs dsh remove
    
  - 支持可选 profile-directory 参数。
  - 写配置前检查文件未被外部修改。
  - 原文件旁生成 .daozai-<uuid>.bak。
  - 使用临时文件后原子 rename。
  - 已有其他插件占用同名包或不兼容 hooks 时拒绝覆盖。

- integrations/agent-notices/bridge/index.mjs
  - 在 DSH 中注册 Stop、PermissionRequest、PreToolUse/request_user_question 相关处理。
  - 审批请求验证工具和完整 operation，只有匹配当前调用时才允许一次。
  - 对带 intent 的原生授权请求保留原客户端语义。
  - questionPayload/validAnswers 对问题 id、选项、答案结构做限制。
  - watchContinued 只观察客户端已进入 continued 的问题，不改变 DSH 的原生工作模式。

- integrations/agent-notices/bridge/channel.cjs
  - Rust Node channel 的子进程入口。
  - descriptor：bridge/live.json，只保存 endpoint、secret、policy 等本机发现信息，不保存请求正文。
  - Windows 使用 \\.\pipe\daozai-<48 hex> 命名管道；非 Windows 使用临时目录 socket。
  - 每行一个 JSON，单帧上限 192 KiB。
  - 使用随机 nonce 和 HMAC-SHA256 做双向握手。
  - 请求连接断开时会输出 cancel，服务退出时删除当前 descriptor。
  - dshReady 只在真实 ready 客户端连接成功后置 true。

- integrations/agent-notices/bridge/notify.cjs
  - Codex hook 的轻量入口。
  - 从 stdin 读取 hook JSON，通过 live channel 把 approval/question/finished 发给岛仔。
  - 岛仔不可用或请求未被接管时退出，让原客户端继续处理。

- integrations/agent-notices/bridge/cordis.patch.yml
  - DSH bundle patch 描述。

- integrations/agent-notices/bridge/package.json
  - @daozai/agent-notices 私有 bundle，type=module。
  - 根目录没有 package.json；Node 测试和依赖入口在 integrations/agent-notices/bridge。

## 5. Agent bridge 协议和状态

Rust channel.cjs 与 agent_cards.rs 之间使用 NDJSON：

- Node -> Rust：
  - dshReady: true/false：DSH 交互插件是否保持 ready socket。
  - id/provider/kind/data：新的 finished、approval 或 question 请求。
  - cancel: <id>：客户端已取消或连接断开。
- Rust -> Node：
  - policy：当前来源开关、interactive、completion、content 等策略。
  - id/reply：对某个请求返回 decision=allow、deny、return 或 decision=answer。
- 结束请求：
  - finished 只展示通知，不进入待处理审批队列。
  - approval/question 进入最多 32 个待处理请求的队列。
  - 队列已满、来源关闭、数据格式不合法或请求超过支持范围时，直接回客户端。

常见 reply：

    {"decision":"allow"}
    {"decision":"deny"}
    {"decision":"return"}
    {"decision":"answer","answers":[...]}

多轮 answers 按原始 questions 顺序保存，每项包含：

    {"id":"question-id","selected":["选项"],"custom":"自定义文本"}

## 6. 多轮问答 UI 的实现约束

当前实现中的 Request 具有：

- question_index：当前显示第几轮。
- question_scroll：当前轮内容滚动位置。
- answers：所有轮次的答案数组。
- details：是否进入详情层。

绘制规则：

1. 详情层只显示当前轮，顶部显示“第 n / total 轮”。
2. 当前轮问题、说明和选项在可滚动区域内。
3. 自定义回答输入框固定在底部，输入框必须真实获得 egui focus。
4. 非多选问题选择选项时清空其他选项和自定义文本。
5. 选择自定义文本时清空非多选选项。
6. 点击“下一轮”前，当前轮必须至少有选项或非空自定义文本。
7. 最后一轮只有在所有轮次都已回答时才允许“提交回答”。
8. 第 1 轮左侧显示“回客户端”；第 2 轮及之后显示“上一轮”。
9. 切换到下一轮时重置该轮滚动位置。
10. 详情层、文本区域和底部按钮不能超出现有岛仔窗口高度；长原因、长 shell 操作和选项数量变化都要通过动态高度或滚动处理。
11. 关闭详情只是收起，不等同于交回；只有明确点击“回客户端”才发送 decision=return。
12. 查看详情或编辑输入时暂停自动交回倒计时。

如果继续修改 UI，优先保持这个状态模型，不要把多轮问题重新压缩成一次性大表单，否则会破坏上一轮返回、答案保留和继续回答语义。

## 7. 配置目录、接入和隐私边界

### 岛仔本地数据

默认目录：

    %APPDATA%\Daozai\agent-notices

其中包括：

- README.md：导出的接入说明。
- connect.mjs：接入/移除脚本。
- bridge/index.mjs、channel.cjs、notify.cjs、cordis.patch.yml、package.json。
- bridge/target.json：当前运行岛仔可执行文件路径。
- bridge/live.json：运行时临时 descriptor，岛仔退出后清理。

### 客户端配置

- Codex 默认目标：
  
      %CODEX_HOME%\hooks.json
  
  未设置 CODEX_HOME 时使用用户目录下 .codex/hooks.json。

- DSH 默认目标：
  
      %DSH_HOME%\profiles\desktop
  
  未设置 DSH_HOME 时使用用户目录下 .dsh\profiles\desktop。

- 额外 DSH profile 可以在接入目录传入 profile-directory 参数。

### 隐私和安全

- 不开放网络 TCP 端口，Windows 双向通道使用本机命名管道。
- live.json 只保存本机发现信息和短期 secret，不保存请求正文或回答。
- 请求、回答、回复内容仅在内存中处理，不写入岛仔通知历史或日志。
- 接入脚本修改原配置前保留 .bak，使用原子替换。
- 不能把 API key、Cookie、授权码、用户消息正文或临时诊断内容写进交接文档。
- 客户端自身的权限、审批生命周期和取消语义优先于岛仔的展示状态。

## 8. 构建环境

项目提供 bundled 编译依赖：

    target/build-tools/env.ps1

它设置 bundled Rust/Cargo、MSVC、Windows SDK、LIB、INCLUDE、CC/CXX。未加载它时，主机 cargo 可能找不到 cl.exe，导致 Windows target 编译失败。

推荐在 PowerShell 中执行：

    Set-Location D:\Codex\daozai
    . .\target\build-tools\env.ps1
    $env:TEMP = "$PWD\target\link-tmp"
    $env:TMP = $env:TEMP

TEMP/TMP 指向 target/link-tmp 是为了避免链接阶段使用不可写或路径过长的临时目录。必要时先创建目录：

    New-Item -ItemType Directory -Force target/link-tmp | Out-Null

正式 release 构建：

    cargo +stable-x86_64-pc-windows-msvc build --locked --release --bin daozai --target x86_64-pc-windows-msvc

运行完整 Rust 回归：

    cargo +stable-x86_64-pc-windows-msvc test --locked --all-targets --features record --target x86_64-pc-windows-msvc -- --test-threads=1

只跑 Agent Rust 测试：

    cargo +stable-x86_64-pc-windows-msvc test --target x86_64-pc-windows-msvc --lib agent_notice -- --nocapture

Node bridge 测试：

    node --test tests/agent-notices.test.mjs

检查补丁空白错误：

    git diff --check

仓库脚本：

- 编译并测试-Claude.cmd：release build + 全部 Rust tests，日志在 target/claude-check/build.log。
- 编译测试版-Claude.cmd：只构建 release，日志在 target/claude-skin/build.log。
- 安装岛仔.cmd：运行 installer/install.ps1。
- 收集岛仔诊断.cmd：运行 scripts/collect-diagnostics.ps1。

如果脚本未自动加载 bundled env.ps1，先在当前 PowerShell 手动 dot-source，再执行 cargo 命令。

## 9. 当前验证证据

本轮已核验：

- Agent 相关 Rust 测试：30/30 通过（agent_notice 12、agent_cards 13、agent_jump 7，含 tray/ui_tests 中的 Agent 用例）。
- Node bridge 测试：11/11 通过。
- 完整 Rust 回归：1317 通过 / 4 失败 / 129 忽略。4 个失败均为本机测试进程中完整性导致的环境限制，与代码无关：`calendar::transport_tests::tls` ×2（写临时文件被拒）、`messenger::qq`（`Shell_NotifyIconW` 被拒）、`quota::api`（凭据目录创建被拒）。
- git diff --check：通过。
- x86_64-pc-windows-msvc release 构建：成功。
- release 文件哈希：590368A700D582172159592333916EC03817AE0C9A29C106C27D01E0FE8992ED。
- 桌面“岛仔－当前测试版”快捷方式目标：指向 `C:\Users\Administrator\Daozai\daozai.exe`。
- 运行中的 daozai 进程路径：指向同一个交付 exe，托盘登记成功。

暂未形成自动化截图/视觉回归基线。审批卡、长文本、多轮问题、自定义输入、跳转入口和不同 DPI/窗口高度仍建议做一次人工验收。

### 本轮新增能力（对照 island-hud 复刻）

| 能力 | 实现位置 | 说明 |
| --- | --- | --- |
| 未知来源/类型请求回交 | `agent_cards.rs` `receive` | 之前静默丢弃会导致客户端卡到宿主超时 |
| 提问契约校验 | `agent_cards.rs` `question_violation` | 题量 1–20、题干 ≤20000 字符、选项 ≤50、label 非空且唯一、题目 id 唯一 |
| 同 id 去重 | `agent_cards.rs` `receive` | 客户端重试不会在队列里叠两条 |
| Provider 表驱动 | `agent_notice.rs` `SITES`/`ProviderMeta` | 新增来源只需改表和设置字段 |
| 会话跳转 | `agent_jump.rs` | Codex 深链精确跳转；DSH 前置客户端窗口，失败回落工作目录；失败在岛上提示原因 |
| 事件透传 sessionId | `bridge/index.mjs`、`bridge/notify.cjs` | 跳转的数据基础 |

`agent_cards.rs` 的 `receive` 中，索引必须在 `remove` 之后计算，否则 VecDeque 越界（已有回归测试 `duplicate_request_id_replaces_existing_entry` 锁定）。

## 10. 实机接入和验收流程

### DSH

1. 在岛仔设置中打开“消息提醒 → AI 助手 → DeepSeek Harness”。
2. 打开“岛上处理审批与提问”。
3. 如需延后回答测试，打开“接收延后提问”。
4. 首次显示“未找到桌面配置”时，先启动一次 DSH，再点击“重试接入”。
5. 完全退出 DSH，包括系统托盘，再重新打开。
6. 设置显示“交互插件已连接”后再测试岛上审批或问答。
7. 使用 DSH 的 ask_user_question 产生单选、多选、自定义和多轮问题。
8. 验证：
   - 自定义输入框可点击并输入。
   - 多轮问题一次显示一轮。
   - 上一轮能返回且答案保留。
   - 最后一轮只有在所有轮次完成后可提交。
   - 关闭详情不会误交回。
   - 选择回客户端后原会话继续接收结果。
9. 审批可以先用拒绝路径复验，不需要实际放行危险操作。

### Codex

1. 打开岛仔设置中的 Codex 来源。
2. 设置显示“已配置”后，按 Codex 客户端提示信任 Daozai hooks。
3. 完全重启 Codex 并新开会话。
4. 触发 PermissionRequest。
5. 在岛仔选择允许一次、拒绝或回客户端。
6. “回客户端”必须让原 Codex 继续处理，不能只关闭岛仔卡片。
7. Codex request_user_input 仍应在 Codex 原生界面作答。

### 进程与托盘

- 启动 release 后，确认桌面岛仔和系统托盘都存在。
- 修改并重新构建后，若旧程序仍在运行，先通过托盘退出或切换到新程序。
- 用进程 Path 确认当前运行的是 release 入口，而不是 target/debug 或临时目录。

## 11. 交付前核对命令

核对快捷方式目标：

    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut("$env:USERPROFILE\Desktop\岛仔－当前测试版.lnk")
    $shortcut.TargetPath
    $shortcut.WorkingDirectory

核对 release 文件和哈希：

    $exe = "D:\Codex\daozai\target\x86_64-pc-windows-msvc\release\daozai.exe"
    Get-Item $exe | Select-Object FullName,Length,LastWriteTime
    Get-FileHash $exe -Algorithm SHA256

核对运行进程路径：

    Get-Process daozai -ErrorAction SilentlyContinue | ForEach-Object {
        Get-CimInstance Win32_Process -Filter "ProcessId=$($_.Id)" |
            Select-Object ProcessId,ExecutablePath,CommandLine
    }

交付条件：

1. release 构建和必要测试通过。
2. 快捷方式 TargetPath 指向上述 release exe。
3. release 哈希已记录。
4. 如果程序正在运行，进程 ExecutablePath 已确认指向同一 release exe。
5. 用户可以直接点击桌面“岛仔－当前测试版”进行测试。

## 12. 常见排障

### 编译时报找不到 cl.exe

先执行：

    . .\target\build-tools\env.ps1

确认：

    Get-Command cl.exe
    cargo +stable-x86_64-pc-windows-msvc --version

再运行 target=x86_64-pc-windows-msvc 的命令。不要只依赖主机默认 cargo。

### Agent 设置显示已收到通知，但不能审批

“已收到通知”只证明事件到达，不证明 DSH 交互插件在线。完全退出 DSH，包括托盘，再重启；确认显示“交互插件已连接”。

### DSH 接入提示找不到桌面配置

先启动一次 DSH，让 %DSH_HOME%\profiles\desktop 生成，再点击岛仔中的“重试接入”。

### Codex 点击回客户端没有反应

先确认：

- Codex hooks 已安装到当前 CODEX_HOME。
- Codex 客户端已完全重启并新开会话。
- 当前请求仍在 Codex 生命周期内。
- 岛仔没有因为关闭来源或连接断开提前交回。
- 接入脚本没有被其他 hooks 修改。

可重新执行 codex remove 后 codex install；安装前会保留 .bak。

### 系统托盘没有图标

确认运行的是 release exe，并检查进程是否仍存在。托盘图标显示受 Windows 通知区域折叠、程序启动异常和旧进程残留影响。优先从任务管理器确认进程 Path，再从托盘展开区域查找岛仔图标。

### Node bridge 测试失败

确保当前目录是 D:\Codex\daozai，使用项目内 bridge 依赖入口运行：

    node --test tests/agent-notices.test.mjs

不要从仓库根目录寻找 package.json；bridge 的 package.json 位于 integrations/agent-notices/bridge。

## 13. 当前工作树改动清单

截至本交接文档生成前，工作树状态如下：

已修改：

- README.md
- THIRD_PARTY_NOTICES.txt
- build.rs
- src/app.rs
- src/extras.rs
- src/lib.rs
- src/main.rs
- src/model.rs
- src/tray.rs
- src/ui_tests.rs

未跟踪：

- assets/ai-icons/codex-app.png
- assets/ai-icons/deepseek-harness.png
- docs/AGENT-NOTICES.md
- integrations/agent-notices/
- src/agent_cards.rs
- src/agent_notice.rs
- tests/agent-notices.test.mjs
- tests/fixtures/

本交接文件 docs/PROJECT-HANDOFF.md 也会成为新的工作树文件。接手 agent 应先对这些文件分组 review，再决定是否拆分提交。

建议的提交拆分：

1. Agent bridge 和接入脚本。
2. Rust agent_notice/agent_cards 及 app/model/tray 接线。
3. UI 测试、Node 测试和 fixtures。
4. 文档、图标和第三方声明。
5. 本交接文档。

提交前必须再次运行 git diff --check、Agent Rust 测试和 Node bridge 测试。

## 14. 后续开发优先级

### P0：维持可交付性

- 修复新增 UI 改动后重新构建 release。
- 每次交付核对快捷方式、哈希和运行进程路径。
- 保持 Node channel 断开时自动回客户端。
- 保持接入脚本的备份、原子写和冲突保护。

### P1：继续验证当前 Agent 交互

- 在不同窗口高度和 Windows DPI 下验证审批详情动态高度。
- 验证长 reason、长 operation、超过 6 个选项时的滚动和按钮位置。
- 验证多轮问题返回上一轮、答案保留和自定义输入。
- 验证 DSH/Codex 同时打开时的队列顺序和来源图标。
- 增加真实客户端复验记录，特别是“提交回答后原会话继续”。

### P2：工程化改进

- 增加 Agent bridge 的协议级回归覆盖。
- 增加不同 DPI/窗口高度的视觉截图基线。
- 为接入、移除、配置冲突和客户端重启建立可重复的 Windows smoke test。
- 将 release 交付核对整理为脚本，减少人工遗漏。
- 在不破坏现有 wire_id 顺序的前提下扩展新状态。

### 不要做的事情

- 不要让岛仔自行伪造 DSH/Codex 的永久授权。
- 不要把请求正文写入通知历史或普通日志。
- 不要用网络端口替代当前本机命名管道协议。
- 不要把 Codex request_user_input 强行搬到岛仔中回答。
- 不要在工作树未核对前 reset/clean。
- 不要新建临时 release 目录绕过桌面快捷方式入口。

## 15. 接手完成标准

新的开发 agent 完成接手后，应能回答并验证以下问题：

- 我知道当前分支、HEAD 和未提交改动范围。
- 我知道 release 程序的固定路径以及桌面快捷方式目标。
- 我能加载 bundled env.ps1 并成功构建 x86_64-pc-windows-msvc release。
- 我能运行 Agent Rust 8/8 和 Node bridge 10/10 测试。
- 我知道 DSH/Codex 的接入目录、配置文件和移除命令。
- 我理解 live.json、HMAC、nonce、NDJSON 和命名管道的作用。
- 我理解多轮问答的 question_index、answers、question_scroll 和上一轮/下一轮规则。
- 我能区分关闭详情、回客户端、拒绝和允许一次的行为。
- 我能按项目规则更新桌面快捷方式入口并核对哈希和进程路径。
- 我不会清理掉当前未提交的 Agent UI/bridge 改动。


