# 开放音乐识别与无时间轴歌词计时

本改动基于上游提交 `f046d6cb972ceeee99fd1c02aafcf6afc8ee6e80`，扩展 Windows SMTC 音乐识别，保留岛仔原有的严格歌词匹配。

## 行为

- 非浏览器播放器的 `PlaybackType=Music` 会话不再依赖软件名称列表。缺少歌手时可显示曲目和控制按钮，但严格查词仍需要完整信息。
- 未能明确分类的会话先保留用于查词，只有严格匹配到含正文的同步歌词后才确认是音乐。纯文本、空歌词、无匹配和网络错误均不能确认。
- 正在播放的未知会话可以进入查词，不被另一个暂停播放器挡住。明确音乐优先于未知会话；浏览器保留视频标题和专辑信息检查。
- 岛体、音乐页面、侧边停靠与宠物采用相同的显示判定。确认前不显示音乐卡片或开放控制按钮。
- 歌词确认绑定来源、歌名、歌手、曲长和已知歌曲 ID；切歌和切换来源重新确认，刷新、暂停或关闭歌词显示保留确认。
- `src/lyrics.rs` 未修改，没有引入“时长相近即可绕过歌手校验”的逻辑。

## 缺少时间轴时的处理

部分播放器能提供歌名、歌手和播放状态，但 SMTC 的播放位置、总时长和更新时间全部为零。此前岛仔能取得歌词，却始终按第零秒显示，导致歌词停在署名行。已在 Muyu 的媒体会话中确认这种情况。

参考 [NetSpeed-Dynamic 的本地计时](https://github.com/GEORGEWWWU/NetSpeed-Dynamic/blob/cea9ddd3d55de58ff9066527d0fbdbe196a2f2df/src/views/WidgetIsland.vue)，增加以下处理：

- 没有时间轴时，从首次识别当前歌曲起累计播放时间，推动同步歌词；暂停时冻结，恢复后续走。
- 来源、歌名、歌手、专辑或双方已知且不同的歌曲 ID 改变时重置计时。SMTC 和本机读取交替导致 ID 暂缺，不算切歌。
- 真实时间轴出现时优先使用实际位置，包括跳转到开头；真实时间轴暂时消失时，从最后位置继续计时。
- 展开页用 `≈` 标记估算的播放位置；拖动预览显示目标时间，不添加估算标记。
- 未知总时长继续显示 `--:--`，不根据歌词末尾编造曲长，也不把估算值送入严格查词或改变歌词缓存键。

**限制**：没有 SMTC 时间轴且没有下述专用进度来源时，中途启动岛仔、拖动进度或同歌循环后，无法恢复精确的真实位置。本地计时解决歌词停在零秒的问题。未注册 SMTC 会话的软件和 NetSpeed 的 JustSolo WebSocket 路径不在本次范围内。

## Muyu 手动拖动后的校准

Muyu 1.1.9 的原生播放器在安装目录旁的 `independent-data/native-ui.json` 保存当前播放位置，拖动进度后也会更新。岛仔在 SMTC 缺少时间轴时，只读这个固定文件的播放字段，并通过歌名、歌手和可用专辑信息核对当前歌曲，从而跟随后退、快进、拖回开头，以及中途启动时的实际位置。

- 文件路径由运行中的 `Muyu.exe` 安装位置发现，不扫描账号文件，不修改 Muyu 数据。
- 只支持已确认的播放格式版本；限制文件大小，校验位置和时长范围，播放中拒绝超过五秒的旧快照及未来时间戳。出现格式变化、数据缺失或身份不一致时回退到已有计时。
- 暂停时使用保存的位置，恢复时不将暂停时间算入播放进度；读取的新真实位置优先于本地估算。
- 不将华为歌曲 ID 当作网易云 ID，也不放宽 `src/lyrics.rs` 的严格匹配。
- Muyu 写文件有延迟，拖动后可能需要数秒完成校准；这不是播放器直接推送的实时进度。其他不提供 SMTC 时间轴的播放器仍需要自己的可验证进度来源。

实现依据：[Muyu 的保存与跳转逻辑](https://github.com/kylin1104/muyuhuaweimusic/blob/bbf9ce96cbb377ee78f3b3e2c058b54f66374c2f/native_model.go)。当前适配使用程序旁默认数据目录；自定义数据目录不在自动发现范围。

## 构建与自动测试

需要 Windows、Rust stable、MSVC C++ Build Tools 和 Windows SDK。在仓库根目录运行：

```powershell
$env:RUST_TEST_THREADS = '1'
cargo test --locked --all-targets --features record
cargo build --release --locked --bin daozai
```

专项测试：

```powershell
cargo test --locked --lib --features record ui_tests::music_
cargo test --locked --lib --features record muyu_timeline
cargo test --locked --lib --features record unknown_media
cargo test --locked --lib --features record smtc::tests::
cargo test --locked --lib --features record lyrics::tests::matching
```

2026-11-11 验证：Windows x64、Rust 1.99.1、锁定依赖，在 Windows 桌面环境串行执行完整离线回归，14 组结果合计 **785 通过、0 失败、86 忽略**。忽略项是仓库原有的真实账号、实机操作、长时间观测或按需导出测试，不计为通过。音乐页专项 **14 通过、0 失败**，Debug 构建成功。覆盖无时间轴时的歌词换句、暂停续播、身份变化归零、真实跳转接管、估算标记和拖动预览。

`git diff --check` 通过，`src/lyrics.rs` 与上游参考副本一致。用户已在 Muyu 实机确认修复有效；其他播放器和中途跳转场景仍需分别验证。

新增 Muyu 真实进度适配后的最终回归：**793 通过、0 失败、87 忽略**，Muyu 专项 **8 通过、1 忽略**。另行显式运行的只读实机测试在暂停和播放状态下均通过，确认能读取实际位置、总时长及连续推进。快进、后退、拖回开头和相同大小/时间戳文件更新由自动回归覆盖；新适配的实际拖动听感仍可按下表验收。标准 `cargo build --release --locked --offline --bin daozai` 构建成功，不启用 `record` 演示功能。Release 使用仓库原有 `opt-level=3`、LTO、单代码生成单元和 strip 配置。

## 手动验收

先从托盘退出其他岛仔实例，再启动新程序。需要隔离数据时可在 PowerShell 中运行：

```powershell
$env:DAOZAI_DATA_DIR = Join-Path $env:TEMP 'daozai-open-media-check'
.\target\debug\daozai.exe
```

| 场景 | 预期 |
| --- | --- |
| 已支持 Muyu 播放新歌、快进、后退、拖回开头 | 读取保存的真实位置，数秒内歌词跟随新位置 |
| 其他无时间轴播放器从头播放新歌 | `≈` 后的秒数前进；前奏后歌词继续换句 |
| 暂停十秒后恢复 | 暂停冻结，恢复续走，不算入暂停时间 |
| 切歌或切换来源 | 清除旧计时和不再匹配的歌词 |
| 有真实时间轴的播放器拖动进度 | 位置和歌词跟随实际跳转，包括拖回开头 |
| 保留另一个暂停播放器 | 不阻挡当前播放的候选会话 |
| 浏览器播放带 B站或 YouTube 标记的视频 | 不误显示为音乐，不开放音乐控制 |
| 纯文本歌词、查无歌词或网络失败 | 不能凭失败结果把未知内容确认为音乐 |

诊断当前媒体会话：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\inspect-media.ps1 -Samples 3
```

脚本只读原始 Windows 媒体会话，不显示 Muyu 专用读取补充的位置，不修改播放器或播放状态，也不会自动上传数据。输出包含当前歌名、歌手、播放状态和时间轴，反馈问题时只需提供目标播放器的相关记录。
