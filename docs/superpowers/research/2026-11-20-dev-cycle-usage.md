# dev-cycle 使用说明（2026-11-20）

`scripts/dev-cycle.ps1` 是「改代码 → 构建 → 测试 → 交付 → 跑 E2E → 提交」的可分段入口：每步先做前置校验并给人话错误，
结果写入 `target/dev-cycle/<时间戳>.json`（同目录 `.log`，`<时间戳>/` 下是原始日志）。**不自动 push**。
真实 Build 会停本仓库 exe 启动的岛仔（别家副本只警告、不停）；Deliver/E2E 遇到别家实例报 12 让你手动处理。
下面 `...` 都指 `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\dev-cycle.ps1`。

## 常用命令

```powershell
... -Step SelfCheck                          # 自检（不构建不启停）
... -Step All -Message "修复提问卡片进度"    # 完整链路，含提交；不 push
... -Step Build                              # 只构建
... -Step Verify -Fast                       # ui_tests + agent + node
... -Step E2E -Scenario all                  # 只跑 E2E
... -Step Deliver                            # 只启动并核对交付
... -Step All -DryRun                        # 只看计划：只读校验 + 打印命令
```

## 参数

| 参数 | 作用 |
| --- | --- |
| `-Step` | `SelfCheck/Build/Verify/Deliver/E2E/Commit/All`（逗号或空格多选）；`All` = Build,Verify,E2E,Deliver，有 `-Message`/`-IncludeCommit` 时追加 Commit |
| `-Fast` | Verify 只跑 `cargo test --lib -- ui_tests`、`-- agent` 与 node 测试 |
| `-Scenario` | E2E：`all/approval/question/question-multi/question-multi-turn/done` 或 `done-repro/jump-repro/scroll-repro/banner-jump-repro` |
| `-Message` | 提交 subject（并让 `-Step All` 追加 Commit） |
| `-NoStart` | Deliver 只核对哈希/快捷方式，不启动岛仔 |
| `-DryRun` | 只做前置校验并打印命令；不构建、不停/启岛仔、不测试、不提交 |
| `-IncludeCommit` | `-Step All` 时强制包含 Commit（无 `-Message` 用默认文案） |

## 每步做什么

- **SelfCheck**：自身 BOM（`EF BB BF`）、语法解析、`scripts\*.ps1` BOM 体检、文件/工具链、`Get-Sha256`/`Test-ExeLocked` 自测、实例与快捷方式画像、提交模板试渲染。
- **Build**：停受管岛仔 → `. scripts\dev-msvc-env.ps1` → `cargo build --locked --release --bin daozai --target x86_64-pc-windows-msvc` → 记录 exe 路径/大小/时间/SHA256。
- **Verify**：加载 MSVC 环境（`cargo test --lib` 要链接器）→ `cargo test --lib`（`-Fast` 跑两个过滤器）+ `node --test tests\agent-notices.test.mjs`。
- **E2E**：查锁屏（LogonUI）与单实例 → 调 `agent-e2e.ps1` 或 `agent-e2e-repro.ps1` → 取最新 `docs/superpowers/research/agent-e2e-shots/<时间戳>/summary.json`。
- **Deliver**：核对 exe 哈希与桌面快捷方式 → 停受管实例 → 启动 → 核对进程路径与文件 SHA256 一致。
- **Commit**：`git add -A` → 展示暂存清单 → `git commit -m <subject> -m <自动正文>`（带 exe SHA256、测试摘要、E2E 证据、记录路径）；**不 push**。

## 常见错误

| 退出码 | 含义 | 处理 |
| --- | --- | --- |
| 11 | 锁屏（LogonUI） | 解锁桌面再跑；DryRun 也会拦 |
| 12 | 非本仓库 exe 启动的岛仔（Deliver/E2E） | 手动关掉（脚本不替你停别人的程序）；Build 只警告不停 |
| 13 | exe 被占用且停不掉 | 查杀毒/句柄占用后再构建 |
| 14 | 构建失败 | 看 `<时间戳>/build.log` 尾部和提示 |
| 15 | 测试失败 | 看 `verify-cargo-*.log` / `verify-node.log` |
| 16 | 交付校验失败 | 启动即退/路径不符/哈希不符；先手动运行看报错 |
| 17 | E2E 失败 | 看 `<时间戳>/e2e-<场景>.log` 与证据目录截图/OCR |
| 18 | 提交失败 | 多为 git user.name/email 未配置或有冲突 |
| 19 | 脚本缺 UTF-8 BOM | 按输出里的修复命令重写为 `UTF8Encoding($true)` |
| 20 | 参数不合法 | 按提示修正 `-Step`/`-Fast` 组合 |
| 21 | 缺构建产物 | 先 `-Step Build` |

## 注意

- 脚本必须 **UTF-8 with BOM**；用会丢 BOM 的工具改完先跑 `-Step SelfCheck`（5.1 下丢 BOM 直接语法报错，19 是 7.x 兜底）。
- `-Step All` 顺序为 Build → Verify → E2E → Deliver → Commit：E2E 要独占单实例互斥体、会停掉正在跑的岛仔，交付放最后；只跑 `-Step E2E` 结束后不会自动拉起岛仔（需要就再 `-Step Deliver`）；与别的 agent 共用 target 时 cargo 会抢锁，脚本会警告 cargo/rustc 进程。
