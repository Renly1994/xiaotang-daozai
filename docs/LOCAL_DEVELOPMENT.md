# 本机开发

项目基于上游 `main` 提交 `b93dd84`（版本 `0.6.50`），在本地开发 Steam 功能。

在项目目录使用 PowerShell 7：

```powershell
./scripts/dev-local.ps1 Check
./scripts/dev-local.ps1 Build
./scripts/dev-local.ps1 Test
./scripts/dev-local.ps1 Run
```

8 GB 内存或同时运行较多软件时可用 `./scripts/dev-local.ps1 Build -LowMemory`（`Test`、`Run` 同样支持）。该选项对当前包关闭调试信息和增量编译、使用单并发，并将代码生成拆分为较小单元，不修改系统页面文件设置。

运行前先从托盘退出当前岛仔。原版的单实例锁与数据目录无关，不能同时运行已安装版和开发版。

## 工具和数据

- Rust stable 1.99.1：`target/dev-tools/cargo`、`target/dev-tools/rustup`。脚本只设置当前进程环境，不修改系统 PATH。
- MSVC：现有 Visual Studio 18 Build Tools 中补装的 x64/x86 C++ 组件。
- Windows SDK：Microsoft 官方 NuGet 包 `Microsoft.Windows.SDK.CPP` 和 `Microsoft.Windows.SDK.CPP.x64`，版本 `10.0.26100.8249`，解压到 `target/dev-tools/windows-sdk-cpp` 与 `target/dev-tools/windows-sdk-x64`。系统盘完整 SDK 安装因空间不足已回滚，改用 E 盘本地 SDK。
- 开发数据：`target/dev-data`，不覆盖已安装版的 `%APPDATA%/Daozai`。
- 运行副本：`target/dev-run/daozai.exe`，避免 Windows 对运行中 EXE 的锁阻止后续构建和测试。
- 临时目录：`target/dev-temp`。默认 Cargo 编译并发为 2，避免全目标编译耗尽提交内存；可通过 `CARGO_BUILD_JOBS` 覆盖。测试线程为 1，与仓库贡献指南一致。

开发脚本将本地 SDK 的 `rc.exe` 路径传给 `RC`。构建脚本支持 `RC`、PATH 和系统 SDK 目录，程序及测试 EXE 都会嵌入图标资源。

`Build`、`Test` 默认使用已下载的离线依赖。未来新增依赖时，在同一个 PowerShell 会话执行 `./scripts/dev-local.ps1 Check` 后，再执行 `cargo fetch --locked`。

上述工具、凭据、开发数据、日志和构建产物都在 Git 已忽略的 `target` 下，不应提交到仓库。`target/dev-tools` 同时存有本地工具链，因此此环境下不要用 `cargo clean` 或直接删除整个 `target`；需要清理构建缓存时仅清理 `target/debug`（先退出开发版）。

Steam 功能需求与接入边界见 [STEAM_INTEGRATION_PLAN.md](STEAM_INTEGRATION_PLAN.md)。

## 原版验证记录（2026-11-05）

- 原版构建通过；运行副本启动成功，日志确认窗口定位完成，使用 Intel UHD Graphics / OpenGL 3.3。
- 原版库测试执行结果：498 通过、2 失败、48 跳过；因库测试失败，全目标命令未继续完成其余目标。完整日志：`target/test-original-final.log`。
- `calendar::tests::discovery_root_errors_follow_well_known_principal_and_home`：测试中的本机模拟服务器读取 socket 时遇到 Windows `WouldBlock`（10035）后直接 unwrap，导致工作线程退出。这是原版测试在本机的失败，不是 Steam 改动引入。
- `tray::tests::icon_resource_is_embedded`：原版构建脚本无法搜索 E 盘 SDK，测试 EXE 没有嵌入资源 #1。运行副本已通过开发脚本单独链接资源，文件版本与产品版本均为 0.6.50。
- 首次测试并发过高还出现过系统提交内存不足（1455）；后续已降低编译并发。应用运行副本避免了测试重建 EXE 时的文件占用问题。
- 上述为 Steam 开发之前的原版基线记录。Steam 开发中补齐了 SDK 资源编译器路径支持，并把日历模拟服务器接收到的 socket 显式设为阻塞模式；修复仅涉及构建与测试兼容。
