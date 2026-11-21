//! 桌面退出工具 —— 独立文件，避免 shell 与 ui 循环依赖。
import 'dart:io';
import 'package:window_manager/window_manager.dart';

/// 退出应用：关窗 + 退出进程。
/// 桌面悬浮窗无标题栏关闭按钮，需显式退出入口（右键菜单 / 面板关闭按钮 / Dock 右键）。
Future<void> quitApp() async {
  try {
    await windowManager.close();
  } catch (_) {
    // 关窗失败也强制退出进程，避免留下关不掉的幽灵窗口。
  }
  exit(0);
}