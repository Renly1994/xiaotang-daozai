//! 对话框临时扩窗 —— 独立文件，避免 shell 与 ui 循环依赖。
//! 日期/时间选择器约 500px 高，岛体窗口常态 220px 会裁掉；
//! 弹前扩到 560×560，关闭后恢复，并钉回原左上角防跳位。
import 'dart:io' show Platform;
import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

const _normalSize = Size(560, 220);
const _dialogSize = Size(560, 560);

bool get _isDesktop => Platform.isWindows || Platform.isMacOS || Platform.isLinux;

/// 桌面端弹对话框前临时扩窗，关闭后恢复；移动端直接执行。
Future<T> withDialogRoom<T>(Future<T> Function() body) async {
  if (!_isDesktop) return body();
  final pos = await windowManager.getPosition();
  await windowManager.setSize(_dialogSize, animate: false);
  await windowManager.setPosition(pos);
  try {
    return await body();
  } finally {
    await windowManager.setSize(_normalSize, animate: false);
    await windowManager.setPosition(pos);
  }
}