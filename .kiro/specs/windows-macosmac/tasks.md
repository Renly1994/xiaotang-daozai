# 任务清单：跨端文件置物架（无服务器 · 端到端加密）

- Spec：`windows-macosmac` · 阶段：Tasks
- 依据：`requirements.md`（R1–R7、N1–N10、D1–D5）+ `design.md`（§1–§7）
- 执行顺序：T1 → T2 → T3 →（T4 可选，D2 届时定）。每片收尾过门禁才进下一片。
- **交付流程（用户指定 2026-11-22）**：每片代码完成 → 本地运行给用户看效果 → 用户确认通过 → **用户手动 git push 到 GitHub**（本机无凭据，N5；AI 不代推、不建 PR）。本地运行依赖临时工具链（见 T1.0），与 HANDOFF §5.2 的会话 scratch 安装同模式。

门禁定义（每片收尾统一执行）：
1. `flutter analyze --no-fatal-infos` exit 0（warning 即致命，HANDOFF §5.2 教训）
2. `flutter test` 全过（既有 36 + 本片新增，零回归 R7.4）
3. 本地真跑：桌面端 `flutter run -d macos`（或 windows）功能走查给用户看
4. 用户口头通过后，用户手动 `git push origin main` 触发 CI 云端构建复核（T1 桌面两端，T2 起三端）
5. CI 绿 + 该片真机验收清单（requirements §8 对应条目）过

---

## T1 双机直连薄片（桌面↔桌面，手动 IP 配对）

### T1.0 环境与依赖基线
- [ ] T1.0.1 会话 scratch 安装 Flutter stable（含 Dart），`flutter --version` 记录版本号；不污染系统持久环境（HANDOFF §5.2 同模式）。本机 macOS arm64 可跑 macos 桌面端；Windows 端验证走 CI + 用户 Windows 真机。
- [ ] T1.0.2 `flutter create --platforms=macos,windows --org com.dynamicisland --project-name dynamic_island .` 生成 runner（不入库，N2）；`rm -f test/widget_test.dart`；`bash ci/setup_platform.sh macos`。
- [ ] T1.0.3 pubspec 新增依赖并**锁精确版本**（N3）：sqflite、sqflite_common_ffi、cryptography、flutter_secure_storage、desktop_drop、uuid、path_provider、crypto（SHA-256）。逐个核对当前稳定版 API 签名后再写调用代码；`flutter pub get` 解析通过。
- [ ] T1.0.4 基线验证：`flutter analyze --no-fatal-infos` exit 0 + 既有 36 测试全过，然后才动功能代码。
- [ ] T1.0.5 macOS entitlements 注入脚本扩展（ci/setup_platform.sh macos 分支）：追加 `com.apple.security.network.server` + `com.apple.security.files.user-selected.read-write`（DebugProfile 与 Release 两份）。验证：脚本 bash -n 过 + 生成 entitlements plutil 解析过。

### T1.1 纯逻辑核心（无平台依赖，先行+单测）
- [ ] T1.1.1 `core/shelf_index.dart`：ShelfEntry 模型（design §2.3 全字段，trashed/version T1 预留）、toJson/fromJson、去重判定（sha256+relPath）、搜索过滤（大小写不敏感子串，中文原样）。
  - 测试 `test/shelf_index_test.dart`：去重命中/不同路径同内容不误判/搜索大小写/中文子串/空关键词返回全部/JSON 往返。覆盖验收 1、3、13。
- [ ] T1.1.2 `core/protocol.dart`：帧编解码 `[1B type][4B len][payload]`、消息类型常量（0x01–0x0B）、版本协商、indexDelta/blobRequest/blobChunk/blobDone/ack/error/ping 载荷序列化。
  - 测试 `test/protocol_test.dart`：每类消息编解码往返/截断帧拒收/超长帧拒收/版本不匹配拒绝。
- [ ] T1.1.3 `core/pairing.dart`：配对状态机（idle→advertising→connecting→keyex→sasShown→confirmed/failed）、一次性 token 生成与校验、120s TTL、SAS 计算接口（共享秘密→6 位码）、错码/超时回 idle 无残留。
  - 测试 `test/pairing_test.dart`：token 重放拒绝/过期拒绝/错 SAS 拒绝/超时回 idle/正常流转到 confirmed/半配对状态清理。覆盖验收 6。
- [ ] T1.1.4 `core/chunker.dart`：分块计划（256KB）、offset 计算、SHA-256 流式校验器封装。T1 只用整文件校验；offset 逻辑写好但 T2 才接续传。
  - 测试 `test/chunker_test.dart`：分块边界（恰好整除/余 1 字节）/校验一致与不一致。

### T1.2 端口与存储实现
- [ ] T1.2.1 `services/ports.dart` 扩展：ShelfStore/BlobVault/DownloadExport/PeerTransport/PeerLink/Discovery/KeyVault/FileIntake 抽象（design §2.2 原文签名）。既有 TodoStore/ReminderNotifier/Clock 不动。
- [ ] T1.2.2 `services/sqlite_store.dart`：sqflite 实现 ShelfStore；文件名/relPath 用 vault 主密钥 AES-GCM 加密存 BLOB；upsert 按 sha256+relPath 幂等；桌面端 sqflite_common_ffi 初始化（Windows 需 sqlite3.dll，CI 构建脚本核对）。
  - 测试 `test/sqlite_store_test.dart`（ffi 内存库）：upsert 幂等/加密字段落盘非明文/读取解密还原/remove。
- [ ] T1.2.3 `services/file_vault.dart`：AES-256-GCM 落盘（随机 nonce 前置）、临时名+原子改名、get 解密流、usedBytes、启动清扫临时名残留。
  - 测试 `test/file_vault_test.dart`（临时目录）：put→get 内容一致/篡改密文拒绝/半文件不被当完整/清扫残留。覆盖验收 1（副本入库）。
- [ ] T1.2.4 `services/secure_keys.dart`：flutter_secure_storage 实现 KeyVault（Ed25519 身份+X25519 静态密钥 loadOrCreate、信任表增删查、vault 主密钥）。测试用 fake 内存后端（端口注入），真机 Keychain/DPAPI 走查。
- [ ] T1.2.5 `services/download_export.dart`：DownloadExport 实现——平台默认目录探测（Win 安装目录可写性→回落 LOCALAPPDATA；mac Application Support；Android 外部私有目录 T2 接）、export 按 relPath 还原+原子改名、缺失重导、pickDirectory/setBaseDir/resetToDefault（桌面系统目录选择器；mac security-scoped bookmark 持久化）、失效回落默认+可见提示。
  - 测试 `test/download_export_test.dart`（fake vault+临时目录）：导出路径还原/手动删后重导/自定义切换不搬家/不可写拒绝设置/失效回落。覆盖验收 15、16。

### T1.3 加密通道与同步引擎
- [ ] T1.3.1 `services/lan_transport.dart`：dart:io TCP 监听/连接；四消息握手（hello→keyex→auth→ready，design §2.4）：双 ECDH（临时+静态）→HKDF 派生双向会话密钥→transcript Ed25519 验签→XChaCha20-Poly1305 帧加密（nonce 计数器，AAD=帧序号）；10s 握手超时；未信任且无 token 立即断开。
  - 测试 `test/lan_transport_test.dart`（127.0.0.1 回环双实例）：握手成功/验签拒伪/帧篡改 AEAD 拒绝/未信任拒绝/超时断开/SAS 两端一致。覆盖验收 7、8。
- [ ] T1.3.2 `services/sync_engine.dart`：编排——配对会话管理、连接仲裁（指纹字典序留一条）、ready 后 indexSyncRequest 水位补差、收纳/删除即时 indexDelta 推送、拉取队列（并发 2、指数退避 1s/4s/16s×3）、整文件传输+SHA-256 校验通过→vault→download 导出、失败红条事件。T1 无 mDNS：连接目标来自设备页手动 IP。
  - 测试 `test/sync_engine_test.dart`（回环双引擎+fake 存储）：配对全流程/索引水位补差/拉取后两端内容一致且 download 落地/校验失败拒收不入库/未信任拒绝/断连重试。覆盖验收 9（桌面双机版）。
- [ ] T1.3.3 `state/sync_state.dart`：ChangeNotifier——设备列表/在线态/传输进度/收纳摘要（成功 N/重复 N/失败 N）/notice 红条/dropHot 投放态/下载位置当前路径。模式对齐 AppState.reminderNotice（R7.1 不静默）。
  - 测试 `test/sync_state_test.dart`：收纳摘要计数/失败 notice 可见且成功清除/进度更新通知。

### T1.4 收纳入口与 UI
- [ ] T1.4.1 `services/file_intake.dart`：desktop_drop 封装 FileIntake（drops 流）；单文件收纳管线：读原件→流式 SHA-256→去重→磁盘预检（vault+download 双份估算）→vault.put→索引 upsert→摘要事件；超限（默认 512MB）/不可读拒绝且同批继续；**原件不动**。文件夹拖入 T1 提示「文件夹支持将在下一版提供」并拒绝（不静默吞）。
  - 测试 `test/file_intake_test.dart`（fake 端口+临时文件）：收纳成功原件仍在/去重提示/超限拒绝/失败计数/磁盘不足拒绝。覆盖验收 1、3、4。
- [ ] T1.4.2 `ui/drop_overlay.dart` + island_widget 集成：onDragEntered→dropHot→强制展开+cyan 描边+「松手收纳」；拖拽期 setIgnoreMouseEvents(false)，结束恢复穿透判定；收起态直接拖入可收纳。
  - 测试 `test/island_drop_test.dart`（mock window_manager，沿用 island_desktop_test 模式）：拖入展开/离开恢复穿透/投放态视觉标志。覆盖验收 5。
- [ ] T1.4.3 `ui/files_page.dart`：搜索框+4 行列表（24px 行高对齐待办页）+三态图标（本地/远端/下载中）+设备归属角标+空状态引导；点击本地文件→打开/在文件夹中显示（桌面 Process.run open/explorer）；点击远端→触发拉取+进度；IslandPage 枚举加 files，_PageDots 自动多一页。
  - 测试 `test/files_page_test.dart`：搜索过滤/空状态文案/远端标注/下载中态。覆盖验收 13、R5.6。
- [ ] T1.4.4 `ui/devices_page.dart`：已配对设备卡片（名/指纹/在线态）+「添加设备」（手动输 IP:端口→SAS 两端比对→双方确认）+移除（二次确认）+**下载位置**（当前路径/更改目录/打开目录/恢复默认，D5）+同步总开关（R7.2）。IslandPage 加 devices。
  - 测试 `test/devices_page_test.dart`：配对流程 UI 状态/移除二次确认/下载位置显示与失效态/开关停止连接。覆盖验收 6、12、16。
- [ ] T1.4.5 `shell/desktop_shell.dart` 修改：右键菜单加「文件」「设备」入口；main.dart 注入 SyncState（MultiProvider）；退出 dispose 顺序=停连接→关监听→落盘水位（R7.3）。既有右键「关于/退出」不动。
- [ ] T1.4.6 面板几何核对：419×140 内文件页/设备页不溢出（搜索框 26+4×24+圆点）；设备页内容超一屏时内部滚动。dialog_room 扩窗机制复用核对（配对 SAS 对话框不被 220px 裁掉）。

### T1.5 T1 收尾门禁
- [ ] T1.5.1 `flutter analyze --no-fatal-infos` exit 0；`flutter test` 全过（36 既有 + T1 新增，预计新增 ≥40 用例）。
- [ ] T1.5.2 本地真跑给用户看：`flutter run -d macos`——拖单文件入岛→摘要→文件页搜到→打开；设备页改下载目录→新拉取落地新目录；两个本机实例（不同端口+不同数据目录）回环演示配对→同步→拉取全链路。
- [ ] T1.5.3 用户确认通过 → 用户手动 `git push origin main` → CI Windows+macOS 构建绿（含 sqflite_common_ffi sqlite3.dll 核对）。
- [ ] T1.5.4 真机验收（用户 Windows 机 + Mac 同 Wi-Fi）：requirements §8 第 1、3–9、12–14 条逐条过（第 2 条文件夹属 T2；第 9 条以桌面双机代替 Android）。
- [ ] T1.5.5 更新 HANDOFF.md：T1 改动表+新依赖锁版记录+已知坑（防火墙弹窗/本地网络权限）。

## T2 三端完整薄片（mDNS + 安卓 + 文件夹 + 续传）

### T2.1 发现与配对增强
- [ ] T2.1.1 pubspec 锁版新增 bonsoir、qr_flutter（N3 签名核对）。
- [ ] T2.1.2 `services/mdns_discovery.dart`：bonsoir 实现 Discovery——广播 `_daozai._tcp`（TXT 只含版本+指纹+端口，不含设备名/文件名）；发现已配对设备→通知引擎主动连；安卓 MulticastLock 持有期=广播期。
  - 测试：fake Discovery 注入引擎的上线/下线事件流转；真机 mDNS 走查（CI 无法组播）。
- [ ] T2.1.3 二维码配对：设备页展示 qr（指纹+IP+端口+一次性 token），安卓扫码（mobile_scanner 锁版或系统相机 Intent+解析，选型时核对权限面）→直连配对；与短码通道汇合同一状态机。
- [ ] T2.1.4 macOS Info.plist 注入 NSLocalNetworkUsageDescription+NSBonjourServices（setup_platform.sh）；安卓 Manifest 注入 INTERNET/ACCESS_NETWORK_STATE/ACCESS_WIFI_STATE/CHANGE_WIFI_MULTICAST_STATE（已有跳过）；bash -n + XML/plist 解析验证。

### T2.2 文件夹与安卓入口
- [ ] T2.2.1 file_intake 文件夹递归：保留 relPath 结构、跳过符号链接与不可读文件并计数、逐文件去重/预检/入库、批次摘要（成功 N/重复 N/失败 N/跳过 N）。
  - 测试：嵌套结构还原/符号链接跳过计数/部分失败不中断同批。覆盖验收 2。
- [ ] T2.2.2 `platform/android_files/` Kotlin：SAF 通道（ACTION_OPEN_DOCUMENT 多选 + ACTION_OPEN_DOCUMENT_TREE 文件夹，持久化 URI 权限）+ FileProvider（download/ 打开/分享）；MethodChannel `dynamicisland/files`；setup_platform.sh android 注入 + Manifest provider 声明。
- [ ] T2.2.3 `shell/mobile_shell.dart`：底部导航（岛体卡片/文件/设备），files_page/devices_page 同构复用；「+ 添加」走 SAF；download 默认=外部私有目录 files/download/。
- [ ] T2.2.4 安卓「仅 Wi-Fi 同步」开关（R7.2，connectivity_plus 锁版或平台通道网络类型判断）。

### T2.3 续传、离线与删除同步
- [ ] T2.3.1 断点续传：partial/<sha>.part+已收 offset 持久化；重连 blobRequest 带 offset 续发；blobDone 全文件校验→vault→download；心跳 ping/pong 30s、90s 判离线；设备页「离线·最后在线时间」。
  - 测试（回环）：中途断连→重连续传→内容一致/校验失败拒收/心跳超时离线判定。覆盖验收 10。
- [ ] T2.3.2 删除同步：indexDelta(remove)→各端索引删+vault.delete+download.remove（直接删，回收站 T3）；原件永不触碰。
  - 测试：一端删→对端索引/副本/导出三层皆删。覆盖验收 11（删除部分）。
- [ ] T2.3.3 源设备离线点击文件→明确提示「文件在 <设备名> 上，该设备当前离线」，不假下载（R4.4）。

### T2.4 T2 收尾门禁
- [ ] T2.4.1 analyze exit 0 + 全部测试过（含 T2 新增 ≥20 用例）。
- [ ] T2.4.2 本地真跑给用户看：mac 拖文件夹→本机第二实例搜到→拉取→download/ 结构还原；断网续传演示（回环中断注入）。安卓端本地 `flutter run -d android`（需用户接真机/模拟器）。
- [ ] T2.4.3 用户确认 → 用户手动 push → CI 三端构建绿。
- [ ] T2.4.4 三端真机验收：§8 第 2、9、10 条 + 第 11 条删除同步 + 安卓后台退断连如实提示 + macOS 本地网络权限弹窗 + mDNS 被拦时手动 IP 兜底。
- [ ] T2.4.5 更新 HANDOFF.md（T2 改动+bonsoir 三端差异坑+权限清单）。

## T3 体验加固

- [ ] T3.1 回收站：trashed/trashedAt 启用，删除进回收站 7 天可恢复，clearTrash 超期物理删（vault+download 双层）；文件页回收站入口+恢复操作。测试：到期清理/恢复后两层还原。覆盖验收 11（回收站部分）。
- [ ] T3.2 LWW 冲突副本：version 判定+本端未推送修改检测→冲突副本命名 `(冲突 <设备名> <时间>)` 两条都可见。测试：并发修改不丢数据。覆盖 R4.7。
- [ ] T3.3 大窗模式：`withBigWindow()` 720×560 可停留文件管理视图（多列/批量/占用条），与 dialog_room 共用 _resize+嵌套计数；关闭恢复 560×220 钉回原位。Widget 测试：扩缩恢复/嵌套保护。覆盖 R5.4。
- [ ] T3.4 存储管理：设备页占用统计（vault+download 分别计）、一键清理已下载副本（保留索引，可重新拉取）。
- [ ] T3.5 安卓前台服务：dataSync 类型+常驻通知开关（用户可关）；FOREGROUND_SERVICE_DATA_SYNC 注入；被杀重启水位补差幂等。真机走查国产 ROM 限制如实告知。覆盖 R4.8。
- [ ] T3.6 启动对账：vault 有而索引无→补索引或清扫；download 缺失→按需重导；半文件清扫。测试：杀进程模拟后对账。
- [ ] T3.7 投放区收窄到岛体轮廓（RRect 命中复用 _hitIsland，拖拽期接收区=岛体而非整窗）。
- [ ] T3.8 T3 门禁：analyze+测试全过 → 本地真跑给用户看（回收站恢复/冲突副本/大窗/占用统计）→ 用户确认 → 用户手动 push → CI 三端绿 → §8 第 11 条回收站 + R4.7/R4.8 真机验收 → HANDOFF 更新。

## T4 跨网增强（可选，启动前需用户决策 D2）

- [ ] T4.0 D2 决策：T2/T3 验收后向用户提交实测报告（局域网体验/痛点），用户在 A 公共中继盲转发 / B UDP 打洞 P2P 间选择；未决策前不启动。
- [ ] T4.1 按 D2 实现 RelayTransport 或 P2PTransport（既有 PeerTransport 端口，上层零改动）+ RendezvousDiscovery；密文中继抓包验证只见密文（R6.3）。
- [ ] T4.2 门禁同 T3 + 4G↔家庭宽带真机验收。

---

## 回归测试锚点对照（requirements §8 → 任务）

| 验收条 | 任务/测试 |
|---|---|
| 1 原件不动+入库+SHA-256 | T1.4.1 file_intake_test |
| 2 文件夹结构+跳过计数 | T2.2.1 |
| 3 去重不翻倍 | T1.1.1 + T1.4.1 |
| 4 超限/磁盘不足拒绝 | T1.4.1 |
| 5 收起态拖入 | T1.4.2 island_drop_test |
| 6 配对错码/超时 | T1.1.3 pairing_test |
| 7 未配对拒绝 | T1.3.1 lan_transport_test |
| 8 指纹不一致中止 | T1.3.1 |
| 9 跨端搜到+拉取一致+download 落地 | T1.3.2 sync_engine_test（桌面）→ T2.4.4（三端） |
| 10 断线续传 | T2.3.1 |
| 11 删除同步+回收站 | T2.3.2（删）→ T3.1（回收站） |
| 12 开关停广播 | T1.4.4 devices_page_test |
| 13 搜索 | T1.1.1 + T1.4.3 |
| 14 既有 36 零回归 | 每片门禁第 2 项 |
| 15 download 缺失重导+不可写回落 | T1.2.5 download_export_test |
| 16 自定义目录切换/失效/恢复默认 | T1.2.5 + T1.4.4 |

## 执行纪律（每任务通用）

- 改代码前先读相邻文件对齐风格；不加注释除非解释非显然决策（项目现有中文注释风格保留）。
- 新依赖先锁版+核对签名再写调用（N3）；平台原生改动一律进 setup_platform.sh（N2）。
- 每完成一个子任务即跑对应测试；analyze warning 视同失败（HANDOFF §5.2）。
- 不 git commit/push，除非用户明确要求；push 永远由用户手动执行（N5+用户指定流程）。
- 不引入已删除功能（N6）；不改原 Rust src/。
- 本地运行给用户看效果是每片门禁的必经步骤，用户通过后才进入推送环节。