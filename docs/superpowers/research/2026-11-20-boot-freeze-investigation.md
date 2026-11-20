# 岛仔「进程活着但界面/桥接不工作」排查记录（2026-11-20 夜）

## 现象

交付新构建后（`target\x86_64-pc-windows-msvc\release\daozai.exe`，SHA256 见下），用桌面快捷方式 / `Start-Process` / `ShellExecute` / `WMI Create` 启动，出现：

- 进程存在、窗口「岛仔」存在且 `IsWindowVisible=true`，线程全部处于 `Wait`，CPU 空闲；
- `%APPDATA%\Daozai` 下**没有任何**新写入：没有 `diagnostics\last-startup.log`、没有 `agent-notices\bridge\live.json`、`ai-quota-cache.json` 时间戳不动；
- 用 `DAOZAI_BOOT_TRACE` 打的**第一行都写不出来**（`run: 进入` 在 `main` 里、创建窗口之前）；
- 换成「把 exe 复制到数据目录里再启动」「`DAOZAI_DATA_DIR` 指向自建临时目录」等测试配置时，同一份二进制**完全正常**：`live.json` 秒级出现，`DAOZAI_BRIDGE_TRACE` 能看到 `Cards::update` 每帧日志。

## 已确认的事实（用于下次继续）

1. 同一份二进制在「数据目录 = 自建临时目录」下一切正常，说明不是构建/代码逻辑回归，而是**运行环境差异**。
2. 窗口「岛仔」在**没有任何诊断写入**的情况下就存在了——按 `src/main.rs` 的顺序，`diagnostics::startup()` 在 `eframe::run_native` 之前，若它没执行则窗口不该存在。因此怀疑：
   - 我们看到的是**别的进程**留下的窗口（本次排查里出现过大量残留 `daozai.exe`/`channel.cjs` 子进程，已清理），或
   - 该实例已经走到 `single_instance()` 的「占用」分支（弹出「已在运行」提示后 `return Ok(())`），而窗口来自当时仍存活的旧实例。
3. 杀掉全部残留后重测：`Local\Daozai.SingleInstance` 互斥体 `free`、无 daozai 进程，启动后仍是「窗口在、无任何写入」。**结论尚未闭环**。
4. 用户数据目录被完整备份/还原过一次（排查时临时移走 `%APPDATA%\Daozai`、还原后 14 个条目齐全，`settings.txt` 未丢）。排查中误改写过一次 `diagnostics\last-startup.log`（已删除，程序启动会自行重建）。

## 下次续查的步骤（按顺序，成本低到高）

1. 先确认窗口归属：`GetWindowThreadProcessId(窗口)` 与 `daozai.exe` 的 PID 对齐；若不对齐，说明是残留实例的窗口，先 `taskkill /F /IM daozai.exe` + 杀 `channel.cjs` 子进程。
2. 用**用户的真实启动方式**（双击桌面快捷方式）复现，不要用 `Start-Process`：
   `explorer.exe "C:\Users\59531\Desktop\岛仔－当前测试版.lnk"`，然后 `Get-Process daozai | Select StartTime` 对比。
3. 打开 `DAOZAI_BOOT_TRACE=1`（通过用户环境变量或 `.lnk` 的启动参数）再看 `%TEMP%\daozai-boot-trace.log` 是否出现——若仍为空但窗口存在，则窗口不是该进程的。
4. 若确认是本进程卡在启动期，用 `procdump -ma <pid>` 抓一次全量转储，或 `windbg -p` 看主线程栈（`Shell::new` → `App::new` → 托盘/热键/服务初始化）。
5. 对照检查点：`App::new` 里会读取 `%APPDATA%\Daozai\credentials`（Windows 凭据管理器/DPAPI），若该目录/条目损坏，可能在启动期阻塞或 panic 后再被 `install()` 的 panic 钩子吞掉（钩子会尝试自动重启，可能出现「进程活着但什么都不做」的观感）。**优先验证：临时把 `credentials` 改名后启动。**

## 当前交付状态

- 构建：`target\x86_64-pc-windows-msvc\release\daozai.exe`，22,831,104 字节，2026-11-20 23:05:35，SHA256 `47826E4753FFBA3CF9E2624156E864CF5E10CA1BE344171C321B1327AB1CB040`。
- 该二进制包含（且仅包含）本轮源码改动 + 两个**默认关闭**的排查开关；开关不开时零额外开销、不写任何文件。
- 未解决：上面第 3 条的启动异常。用户侧若遇到「岛仔图标在但卡片不弹」，先看 `%TEMP%\daozai-boot-trace.log`（需 `DAOZAI_BOOT_TRACE=1`）。
