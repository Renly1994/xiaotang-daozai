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

**限制**：完全没有时间轴或切歌标识时，中途启动岛仔、在播放器内拖动进度或同歌循环后，无法恢复精确的真实位置。本地计时解决歌词停在零秒的问题，从头播放是这类播放器的主要验收方式。未注册 SMTC 会话的软件和 NetSpeed 的 JustSolo WebSocket 路径不在本次范围内。

## 构建与自动测试

需要 Windows、Rust stable、MSVC C++ Build Tools 和 Windows SDK。在仓库根目录运行：

```powershell
$env:RUST_TEST_THREADS = '1'
cargo test --locked --all-targets --features record
cargo build --locked --bin daozai --features record
```

专项测试：

```powershell
cargo test --locked --lib --features record ui_tests::music_
cargo test --locked --lib --features record unknown_media
cargo test --locked --lib --features record smtc::tests::
cargo test --locked --lib --features record lyrics::tests::matching
```

2026-11-11 验证：Windows x64、Rust 1.99.1、锁定依赖，在 Windows 桌面环境串行执行完整离线回归，14 组结果合计 **785 通过、0 失败、86 忽略**。忽略项是仓库原有的真实账号、实机操作、长时间观测或按需导出测试，不计为通过。音乐页专项 **14 通过、0 失败**，Debug 构建成功。覆盖无时间轴时的歌词换句、暂停续播、身份变化归零、真实跳转接管、估算标记和拖动预览。

`git diff --check` 通过，`src/lyrics.rs` 与上游参考副本一致。用户已在 Muyu 实机确认修复有效；其他播放器和中途跳转场景仍需分别验证。

## 手动验收

先从托盘退出其他岛仔实例，再启动新程序。需要隔离数据时可在 PowerShell 中运行：

```powershell
$env:DAOZAI_DATA_DIR = Join-Path $env:TEMP 'daozai-open-media-check'
.\target\debug\daozai.exe
```

| 场景 | 预期 |
| --- | --- |
| 无时间轴播放器从头播放新歌 | `≈` 后的秒数前进；前奏后歌词继续换句 |
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

脚本只读 Windows 媒体会话，不修改播放器或播放状态，也不会自动上传数据。输出包含当前歌名、歌手、播放状态和时间轴，反馈问题时只需提供目标播放器的相关记录。
