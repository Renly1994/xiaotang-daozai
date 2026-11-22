# 交接文档（HANDOFF）

> 面向后续接手的开发者 / AI Harness。读完本文即可了解项目全貌、所有改动、当前状态、已知坑与继续开发的方法。
> 最后更新：2026-11-22 · 对应提交 `bdc9dcf`（已推送至 origin/main）

---

## 1. 项目是什么

**Daozai 跨平台版**：基于开源 [岛仔](https://github.com/Renly1994/xiaotang-daozai)（Rust + egui，Windows 专属灵动岛）重做的 **Flutter 多平台桌面伙伴**，目标覆盖 **Windows / macOS / Android**。

- 本工程位于 `daozai_flutter/`，是独立 Flutter 工程，**不修改原 Rust 仓库**（原仓库在上级目录 `src/`，仅作参考）。
- 形态：桌面端透明置顶悬浮岛（悬停展开）；安卓端完整 App + 主屏小组件。
- 当前版本 v0.1.0，**可运行骨架 + 产品级修复**，非功能完整成品。

## 2. 最高开发原则（用户明确要求，必须严格遵守）

1. **功能实用性优先**：每个功能必须解决用户真实场景的问题，不做花架子、不堆砌无用功能。
2. **逻辑正确性优先**：边界条件、异常路径、状态一致性必须正确，宁可少做也不做错。
3. **产品视角 / 用户视角**：开发前先想"用户实际会怎么用、哪里会卡住、哪里不符合直觉"，而不是"技术上能实现什么"。基本体验（退出、设置、反馈、空状态、错误提示）必须完整。
4. **优先级排序**：功能实用 & 逻辑正确 > 规范性 > 美观。

> 教训：v0.1 骨架曾漏掉桌面退出入口（无边框+置顶+skipTaskbar 却无托盘/右键/关闭按钮），造成"关不掉的幽灵窗口"。这是脱离用户视角的典型错误。**任何新功能都要先保证基本体验闭环。**

## 3. 用户原始需求与决策记录

| 需求/决策 | 用户选择 |
|---|---|
| 多平台可安装运行（Win/mac/Android） | ✅ Flutter 重做外壳 |
| 安卓形态 | 小组件 + 完整 App |
| 首期范围 | 先做可运行骨架 |
| 功能取舍（最新） | **保留：时钟常驻 + 待办清单 + 桌面悬浮岛 + 安卓小组件**。已删除：天气、喝水提醒、番茄专注。不再纳入路线：消息提醒、桌宠、音乐歌词、娱乐相关。后续仅扩展 AI 额度/邮箱验证码/日历/剪贴板等纯逻辑实用功能 |
| 打包方式 | CI 云端自动打包（本机无工具链） |
| 桌面交互 | 保留原版悬停展开（移开收起）手感 |
| 命名规范 | 用户可见处统一项目名 Daozai，不用中文品牌称呼；产物名 daozai-平台 |
| 开发原则 | 实用性/逻辑正确性/用户视角优先（见第 2 节） |

## 4. 代码结构

```
daozai_flutter/
├── lib/
│   ├── main.dart                  # 入口：按 Platform 分流桌面/移动
│   ├── core/                      # 从 Rust 复刻的纯逻辑 + 工具（跨平台复用）
│   │   ├── theme.dart             #   配色/字号/几何（← theme.rs, app.rs）
│   │   ├── spring.dart            #   二阶阻尼弹簧（← anim.rs，逐行复刻）
│   │   ├── otp.dart               #   验证码加权打分（← otp.rs，保留供后续邮箱用）
│   │   ├── quit.dart              #   桌面退出（关窗+exit(0)，独立文件避免循环依赖）
│   │   └── window_pos.dart        #   窗口位置持久化（独立文件避免循环依赖）
│   ├── services/
│   │   └── local_store.dart       #   待办持久化（shared_preferences JSON）
│   ├── state/app_state.dart       # 应用状态机：时钟 + 待办（ChangeNotifier）
│   ├── ui/
│   │   ├── island_shape.dart      #   刘海轮廓 CustomPainter（← shape.rs）
│   │   ├── island_widget.dart     #   收起↔展开弹簧形变 + 悬停状态机 + 拖动
│   │   └── pages.dart             #   待办页 + 公共控件
│   └── shell/
│       ├── desktop_shell.dart     #   透明置顶悬浮窗 + 右键菜单（关于/退出）
│       └── mobile_shell.dart      #   安卓完整 App（时钟+待办卡片）
├── platform/android_widget/       # 安卓主屏小组件原生脚手架（Kotlin + res）
├── ci/setup_platform.sh           # CI 平台配置注入脚本
├── test/core_test.dart            # 纯逻辑回归测试（弹簧/验证码）
├── pubspec.yaml
├── README.md                      # 构建运行指南
├── CI_GUIDE.md                    # CI 打包与下载指南
└── HANDOFF.md                     # 本文档
```

> 已删除文件：`core/weather_code.dart`、`services/weather_service.dart`（天气）；
> 喝水/专注的状态、存储、UI 已从 app_state/local_store/pages 中移除。

**关键设计**：
- 时间驱动：岛体形变用 Ticker 每帧驱动复刻的 Spring（半隐式欧拉+子步，帧率无关）。
- 悬停状态机：`_updateHover` 在每帧确定性判断 hover_since/leave_since，避免延迟回调竞态。
- 操作锁定：`AppState.interactionLock`——输入框聚焦/切页时岛体不收起，保证用户来得及操作。
- 循环依赖规避：quit/window_pos 抽到 core/ 独立文件，shell 与 ui 都引 core，不互相引。

## 5. 完整改动历史（按提交，含原因）

| 提交 | 内容 | 原因/根因 |
|---|---|---|
| `338c4d2` | 新增 Flutter 骨架 + 三端 CI | 用户要多平台可安装运行 |
| `c23aba0` | CI 移除默认 widget_test | flutter create 生成的默认测试引用 MyApp，本工程无此类，三端第 10 个测试必挂 |
| `5e06992` | Android 小组件包名动态对齐 | Flutter applicationId 是 com.daozai.daozai_flutter，Provider 写死 com.daozai.flutter 致 R 引用编译失败；改占位符动态读取 |
| `eb40a8f` | macOS 包改标准 zip | ditto --sequesterRsrc 生成 __MACOSX 条目，部分 macOS 归档工具报"格式不支持" |
| `a0e5e76` | 功能 bug + 命名规范 | ① 悬停 Future.delayed 竞态→Ticker 状态机 ② 天气"明天"用 now.day+1 月末越界→日期差 ③ 喝水撤销上限不一致；用户可见"岛仔"→Daozai |
| `533f51e` | 桌面退出入口 | 无边框+置顶+skipTaskbar 却无退出入口，关不掉的幽灵窗口；加右键菜单/关闭按钮/保留 Dock 图标 |
| `9b30a8f` | 产品级致命+逻辑修复 | ① 桌面天气页加城市输入框（原本无处设城市）② interactionLock 操作锁定 ③ 专注阶段结束提示 ④ 喝水目标/单次量可调 ⑤ 待办删除两段式确认 |
| `bdc9dcf` | 体验修复 | ① 移除假电池图标（不显示假信息）② 窗口可拖动+位置记忆 ③ 右键菜单加"关于" |

> 全部已推送至 origin/main（远程 HEAD = bdc9dcf）。

## 6. 当前状态

- **三端 CI 构建成功**（run 44917977836，提交 5e06992）：daozai-windows 11.6MB / daozai-macos 17.3MB / daozai-android 22.8MB。
- 后续修复（a0e5e76~bdc9dcf）已推送，**需触发新一轮 CI 才产出含全部修复的包**。
- macOS 版已帮用户解包安装到 /Applications/daozai_flutter.app（旧版，不含最新修复）。
- 本机（macOS arm64）**无 Flutter/Dart/Rust/Xcode/Android SDK**，无法本地编译；只能交付源码 + CI 云端构建。

## 7. 已知坑与限制（接手必读）

1. **桌面精确点击穿透未做**：岛体窗口外的透明区域仍挡住下层桌面点击。原版用 Win32 命中区+HTTRANSPARENT 实现，window_manager 无逐区域穿透 API，需平台原生通道（Win32 SetWindowRgn/WM_NCHITTEST）。**这是桌面端最后一块基础体验短板。**
2. **真实电量/系统状态未接**：收起药丸仅显示时钟；真实电量需平台原生通道（Win GetSystemPowerStatus / mac IOKit / 安卓 BatteryManager）。
3. **已删除/不再做的功能**：天气、喝水、专注（已删代码）；消息提醒、桌宠、音乐歌词、娱乐（不再纳入路线）。**接手者不要重新引入这些。** otp.dart 验证码逻辑保留供后续邮箱功能复用。
4. **平台 runner 目录不入库**：android/macos/windows 由 CI 里 `flutter create` 生成（.gitignore 已排除）。本地运行需先 `flutter create --platforms=... .`。
5. **安卓小组件数据**：读 shared_preferences 待办计数；后续可经 home_widget 插件或 MethodChannel 推送刷新。
6. **推送认证**：本机 git 无 GitHub 凭据（osxkeychain 空、无 SSH key、无 gh），AI 无法代推，需用户手动 `git push origin main`（用户名+PAT）。
7. **macOS 包双层 zip**：GitHub artifact 外层包装 + 内层 app 包，需逐层解压；未签名需 xattr -cr + 右键打开。

## 8. 后续路线（按优先级，已按用户取舍收窄）

| 优先级 | 任务 | 状态 |
|---|---|---|
| P0 | 验证时钟+待办真机好用 | 待用户推送+试用 |
| P1 | 桌面精确点击穿透 | ✅ 已实现（de398ce）：setIgnoreMouseEvents forward 轮廓穿透，Win/mac 生效，Linux 整窗交互 |
| P2 | 待办本地提醒 | ✅ 已实现（bf8e5bd）：系统级定时通知+岛内横幅，铃铛设置/长按清除 |
| P2 | 邮箱+验证码 | 待做：IMAP 跨平台，凭据进 Keychain/Keystore，otp.dart 已就绪 |
| P3 | AI 额度监控 | 待做：HTTP 逻辑可复用 Rust（flutter_rust_bridge）或 Dart 重写 |
| P3 | 日历日程 / 剪贴板历史 | 待做：纯逻辑实用功能 |

> 明确不做：天气、喝水、专注、消息提醒、桌宠、音乐歌词、娱乐相关。

## 9. 构建与运行

```bash
cd daozai_flutter
flutter create --platforms=windows,macos,android --org com.daozai --project-name daozai_flutter .
rm -f test/widget_test.dart        # 删除引用 MyApp 的默认测试
flutter pub get
flutter test                       # 纯逻辑回归（无需设备）
flutter run -d macos               # 或 windows / android
```

CI 打包详见 `CI_GUIDE.md`；工作流 `.github/workflows/build.yml`（三端矩阵）。
注意：上级目录 `.github/workflows/ci.yml` 是**原 Rust 项目的 Windows 检查**，与 Flutter 无关，每次 push 都会触发（单线程编译慢），可加 paths 过滤限制只在改 Rust 时跑。

## 10. 与原 Rust 项目的对应关系

| 原 Rust 模块 | 本工程 | 复刻方式 |
|---|---|---|
| anim.rs 弹簧 | core/spring.dart | 逐行复刻 |
| theme.rs 配色 | core/theme.dart | 逐像素取色 |
| shape.rs 轮廓 | ui/island_shape.dart | CustomPainter 近似 |
| weather.rs | core/weather_code.dart + services/weather_service.dart | WMO 映射逐行复刻，请求参数一致 |
| otp.rs | core/otp.dart | 加权打分复刻（纯字母分支从简） |
| model.rs | services/local_store.dart | 数据模型对齐，存储改 JSON |
| app.rs 状态机 | state/app_state.dart + ui/island_widget.dart | 精简骨架 |
| native/layered/composition | shell/desktop_shell.dart | window_manager 替代，精确穿透待补 |

## 11. 辅助记录位置

- 项目原则与教训：`.workbuddy/memory/MEMORY.md`
- 逐日工作日志：`.workbuddy/memory/2026-11-21.md`
- CI 监控日志：`.workbuddy/memory/ci-monitor.md`
- 每小时 CI 监控自动化任务 ID：`166a46af-f1a7-4dea-bf27-05d13f855b0a`
- 退出旧版进程：`pkill -f daozai_flutter`

## 12. 许可

原项目原创代码 MIT。本骨架沿用 MIT；复用逻辑时保留原项目版权声明。