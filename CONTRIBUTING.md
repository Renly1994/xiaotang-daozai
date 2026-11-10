# 参与开发

欢迎反馈兼容性问题、修复 bug 或完善使用说明。提交较大的功能前，先开 Issue 说明具体场景与预期行为。

## 本地开发

安装 Rust stable、Visual Studio C++ 构建工具和 Windows SDK，克隆仓库后运行：

```powershell
cargo build --locked --bin daozai
cargo test --locked --all-targets --features record -- --test-threads=1
```

为开发实例指定独立目录，避免覆盖日常使用数据：

```powershell
$env:DAOZAI_DATA_DIR = Join-Path $env:TEMP 'daozai-development'
cargo run --locked --bin daozai
```

常规测试使用合成数据。带 `#[ignore]` 的真实账号测试或截图导出测试须按需要显式运行；不要在公共 CI 中连接私人账号。

## 代码导航

| 目录／文件 | 内容 |
| --- | --- |
| `src/main.rs` | Windows 外壳、系统状态和后台服务衔接 |
| `src/app.rs`、`src/pages.rs` | 状态与主界面 |
| `src/calendar.rs`、`src/calendar_ui.rs` | 日历后台服务、只读 provider、重复日程、月历与列表 |
| `src/calendar/caldav/`、`src/caldav_ui.rs` | DAV XML／HTTP、无损 ICS、双向状态机、投影和资源编辑器 |
| `src/organizer/` | 日历账号、日程、待办、队列和冲突的统一 JSON 事务仓库 |
| `src/mail.rs`、`src/mail_reader.rs` | IMAP、MIME、验证码与邮件阅读 |
| `src/quota*.rs` 及 provider 模块 | AI 额度与连接 |
| `src/native.rs`、`src/system.rs` | 原生窗口、输入、拖放和系统事件 |
| `src/ui_tests.rs` | 无窗口交互与布局回归测试 |
| `src/bin/daozai-record` | 合成数据演示与软件渲染器 |
| `vendor/` | 少量必要的上游补丁，见依赖说明 |
| `installer/`、`scripts/` | 安装、卸载与打包 |

## CalDAV 开发与验证

统一仓库使用不可变 `organizer/state-<generation>.json` 和 `CURRENT` 单提交点。不要向旧 `calendar.json`、`local-calendar.json` 或 `todo.txt` 添加新的保存路径。远端原始 VCALENDAR 是同步主数据，旧 `Meeting`／`TodoItem` 只是投影；编辑必须局部 patch 原资源，禁止从展示实例重新生成整份 ICS。

HTTP 请求不得持有数据仓库锁。请求前校验当前 binding generation、账号、双向开关和目标集合；发送前先持久化 Unknown，成功后重新 GET 服务端正文及强 ETag。创建使用 `If-None-Match: *`，修改／删除使用 `If-Match`，任何冲突策略都不能退回无条件覆盖。

建议验证命令（低内存 Windows 使用单个编译任务）：

```powershell
cargo fmt --all --check
cargo check --locked --all-targets --all-features -j 1
cargo test --locked --all-targets --all-features -j 1 -- --test-threads=1
cargo clippy --locked --all-targets --all-features -j 1 -- -D warnings
```

CalDAV 测试包含无损字段、namespace XML、完整清单／增量失败、隔离事务目录及本机 TCP 假 DAV。故障测试应证明失败不提交空数据、不推进 token、不误删、不误确认后继 revision，而非只检查函数返回成功。凭据测试使用随机测试账号及 Windows 凭据管理器，并清理自己生成的记录。真实服务测试必须另外授权，使用独立测试数据目录，且严格遵守用户授权的资源范围；不得把假 DAV 通过写成 Nextcloud、Murena 或 Tasks.org 兼容验收。

`tests/caldav_live.rs` 中的真实往返测试默认 `#[ignore]`。它只修改自己随机创建并登记的资源；任何非本轮资源出现在上传队列中都会终止，清理前核验 UID 并使用强 ETag。通过环境变量提供 `DAOZAI_CALDAV_LIVE_SERVER`、`DAOZAI_CALDAV_LIVE_USERNAME`、`DAOZAI_CALDAV_LIVE_EVENTS`、`DAOZAI_CALDAV_LIVE_TODOS`，另设 `DAOZAI_CALDAV_LIVE_AUTHORIZED=own-resources-only`。密码由用户安全注入 `DAOZAI_CALDAV_LIVE_PASSWORD`；也可在 Windows 凭据管理器中添加目标为 `Daozai.Caldav.Live.Test` 的普通凭据，测试将只读使用，不保存或输出密码。准备好授权与凭据后运行：

```powershell
cargo test --locked --test caldav_live -j 1 -- --ignored --exact authorized_caldav_roundtrip_only_own_resources --nocapture --test-threads=1
```

测试在正常失败或 panic 后都会尝试对已登记的确切 href 做条件清理，再核对原有资源 href／ETag 和本轮资源是否残留。外部进程强杀或网络中断仍可能使清理失败，日志会报告对应自建 UID；不得通过批量删除集合来补偿。测试读取的原始 ICS 仅存于随机临时目录，结束后清理，不写入源码或提交。

## 提交要求

修 bug 时提供能复现的测试；界面变更附不含私人信息的截图。保持改动聚焦，不把个人配置、授权码、Cookie、编译产物或真实账号数据提交到仓库。

源码贡献按仓库 MIT 许可提交；第三方素材须同时提交准确的来源与许可声明。不要修改或移除 vendored 依赖的原始版权声明。
