//! 桌面窗口位置持久化 —— 独立文件，避免 shell 与 ui 循环依赖。
import 'package:shared_preferences/shared_preferences.dart';
import 'package:window_manager/window_manager.dart';

const _posXKey = 'daozai.window.x';
const _posYKey = 'daozai.window.y';

/// 读取上次保存的窗口位置；无记录返回 null（首次启动用默认顶部居中）。
Future<Offset?> loadWindowPosition() async {
  final prefs = await SharedPreferences.getInstance();
  final x = prefs.getDouble(_posXKey);
  final y = prefs.getDouble(_posYKey);
  if (x == null || y == null) return null;
  return Offset(x, y);
}

/// 保存当前窗口位置（拖动结束后调用）。
Future<void> saveWindowPosition() async {
  try {
    final pos = await windowManager.getPosition();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setDouble(_posXKey, pos.dx);
    await prefs.setDouble(_posYKey, pos.dy);
  } catch (_) {
    // 位置保存失败不影响使用。
  }
}