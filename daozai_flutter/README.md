# 岛仔跨平台版（daozai_flutter）

基于开源 [岛仔](https://github.com/Renly1994/xiaotang-daozai)（Rust + egui，Windows 专属）重做的 **Flutter 多平台版本**，目标覆盖 **Windows / macOS / Android**。

> 本目录是独立的 Flutter 工程，不修改原 Rust 仓库。原项目的精华（弹簧动效、刘海轮廓、配色、天气/验证码纯逻辑）已用 Dart 忠实复刻。

## 现状与边界（务必先读）

- **这是可运行骨架（v0.1.0）**，不是功能完整的成品。已实现：时钟常驻、天气、待办、专注、喝水四页 + 桌面悬浮岛形变 + 安卓完整 App 与小组件脚手架。
- **尚未在本机编译验证**：生成此工程的机器未安装 Flutter/Rust/Xcode/Android SDK。代码按 Flutter 3.22+ / Dart 3.4+ 规范编写，装好工具链后应可直接构建；首次运行如有小问题按报错微调即可。
- 音乐控制/歌词、微信 QQ 提醒、邮箱、AI 额度、日历等 **Windows 专属或高难度功能未纳入骨架**（见「后续路线」）。

## 目录结构

```
daozai_flutter/
├── lib/
│   ├── main.dart                 # 入口：按平台分流桌面/移动
│   ├── core/                     # 从 Rust 复刻的纯逻辑（跨平台复用）
│   │   ├── theme.dart            #   配色/字号/几何（← theme.rs, app.rs）
│   │   ├── spring.dart           #   二阶阻尼弹簧（← anim.rs）
│   │   ├── weather_code.dart     #   WMO 天气映射（← weather.rs）
│   │   └── otp.dart              #   验证码加权打分（← otp.rs）
│   ├── services/
│   │   ├── weather_service.dart  #   Open-Meteo 拉取+缓存（← weather.rs）
│   │   └── local_store.dart      #   待办/喝水/设置持久化（← model.rs）
│   ├── state/app_state.dart      # 应用状态机（← app.rs 精简）
│   ├── ui/
│   │   ├── island_shape.dart     #   刘海轮廓绘制（← shape.rs）
│   │   ├── island_widget.dart    #   收起↔展开弹簧形变（← app.rs morph）
│   │   └── pages.dart            #   天气/待办/专注/喝水四页
│   └── shell/
│       ├── desktop_shell.dart    #   透明置顶悬浮窗（Win/mac/Linux）
│       └── mobile_shell.dart     #   安卓完整 App
├── platform/android_widget/      # 安卓主屏小组件原生脚手架（Kotlin + res）
├── test/core_test.dart           # 纯逻辑回归测试
└── pubspec.yaml
```

## 环境准备

```bash
# 1. 安装 Flutter（含 Dart）：https://docs.flutter.dev/get-started/install
flutter --version          # 需 >= 3.22
flutter doctor             # 按提示补齐各平台工具链

# 2. 各平台额外要求
#    Windows：Visual Studio 2022「使用 C++ 的桌面开发」工作负载
#    macOS：Xcode + CocoaPods（sudo gem install cocoapods）
#    Android：Android Studio + SDK（flutter doctor 会指引）
```

## 生成平台工程并运行

本目录只含 `lib/`、`test/`、`pubspec.yaml` 等源码，**平台 runner 目录（android/ macos/ windows/）需用 Flutter 生成**：

```bash
cd daozai_flutter

# 生成三端平台工程（不会覆盖已有 lib/）
flutter create --platforms=windows,macos,android --org com.daozai --project-name daozai_flutter .

flutter pub get

# 运行（接好对应设备/模拟器）
flutter run -d windows     # Windows 悬浮岛
flutter run -d macos       # macOS 悬浮岛
flutter run -d android     # 安卓完整 App
```

### 桌面透明窗口补充配置

`window_manager` 需要平台侧开启透明支持：

- **macOS**：`macos/Runner/MainFlutterWindow.swift` 里把窗口设为透明无边框（插件 README 有标准片段）；`macos/Runner/DebugProfile.entitlements` 与 `Release.entitlements` 加 `com.apple.security.network.client`（天气联网）。
- **Windows**：`windows/runner/main.cpp` 按 window_manager 文档创建透明窗口；联网无需额外权限。

### 安卓小组件接入

`platform/android_widget/` 是脚手架，生成 android 工程后：

1. `res/layout/daozai_widget.xml`、`res/drawable/widget_bg.xml`、`res/xml/daozai_widget_info.xml`、`res/values/strings.xml` → 合并进 `android/app/src/main/res/` 对应目录。
2. `kotlin/DaozaiWidgetProvider.kt` → `android/app/src/main/kotlin/com/daozai/flutter/widget/`。
3. `AndroidManifest_snippet.xml` 的 `<receiver>` → 合并进 `android/app/src/main/AndroidManifest.xml` 的 `<application>` 内。
4. 小组件读取 Flutter 侧 `shared_preferences`（key 前缀 `flutter.`）显示待办计数；天气数据后续经 `home_widget` 插件或 MethodChannel 推送刷新。

## 构建安装包

```bash
flutter build windows --release      # build/windows/x64/runner/Release/
flutter build macos --release        # build/macos/Build/Products/Release/岛仔.app
flutter build apk --release          # build/app/outputs/flutter-apk/app-release.apk
# 或 flutter build appbundle --release  （上架 Google Play）
```

macOS 分发需自行签名/公证（`codesign` + `notarytool`）；Windows 未签名会提示未知发布者；Android 上架需签名密钥。

## 测试

```bash
flutter test         # 纯逻辑回归（弹簧/天气/验证码），无需设备
```

## 后续路线（按优先级）

| 阶段 | 内容 | 说明 |
|---|---|---|
| 1 | 桌面精确命中穿透 | Win32 `SetWindowRgn`/`WM_NCHITTEST`，复刻原版「岛体外点击穿透」 |
| 2 | 设置页 + 城市搜索 UI | 多城市、和风天气、主题切换 |
| 3 | 音乐控制/歌词 | Win SMTC / mac MediaRemote / 安卓 MediaSession，三端机制各异 |
| 4 | 邮箱 + 验证码 | IMAP 跨平台可行，凭据进 Keychain/Keystore；otp.dart 已就绪 |
| 5 | AI 额度 / 日历 | HTTP 逻辑可复用 Rust（flutter_rust_bridge）或 Dart 重写 |
| 6 | 消息提醒 | 桌面与安卓机制差异极大，属高难度项 |

## 与原项目的对应关系

| 原 Rust 模块 | 本工程 | 复刻方式 |
|---|---|---|
| `anim.rs` 弹簧 | `core/spring.dart` | 逐行复刻（半隐式欧拉+子步） |
| `theme.rs` 配色 | `core/theme.dart` | 逐像素取色 |
| `shape.rs` 轮廓 | `ui/island_shape.dart` | CustomPainter 近似 |
| `weather.rs` | `core/weather_code.dart` + `services/weather_service.dart` | WMO 映射逐行复刻，请求参数一致 |
| `otp.rs` | `core/otp.dart` | 加权打分复刻（纯字母分支骨架从简） |
| `model.rs` | `services/local_store.dart` | 数据模型对齐，存储改 JSON |
| `app.rs` 状态机 | `state/app_state.dart` + `ui/island_widget.dart` | 精简骨架 |
| `native/layered/composition` | `shell/desktop_shell.dart` | window_manager 替代，精确穿透待补 |

## 许可

原项目原创代码 MIT。本骨架沿用 MIT；复用逻辑时保留原项目版权声明。