# 本机修复版交付

修复验证通过后，先覆盖构建产物 `target/x86_64-pc-windows-msvc/release/daozai.exe`，再复制到桌面「岛仔－当前测试版」快捷方式实际指向的 `C:\Users\Administrator\Daozai\daozai.exe`（2026-11-20 核对：快捷方式目标是这个副本，不是 target 下的 exe）。两处保持同一份文件、同一 SHA256；交付前核对快捷方式目标与两处哈希。如果岛仔正在运行，先停止进程再覆盖，然后启动到更新后的程序并核对进程路径。后续修复沿用同一入口，不另建临时交付目录。

构建环境用 `scripts/dev-msvc-env.ps1`（自动探测 VS BuildTools 并钉死 CC/CXX/AR），细节见 `docs/superpowers/research/2026-11-20-agent-ui-polish-delivery.md` §5。
