//! 岛仔跨平台版入口。
//! 桌面（Windows/macOS/Linux）：透明置顶悬浮岛；Android：完整 App + 小组件。
import 'dart:io' show Platform;
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:intl/date_symbol_data_local.dart';
import 'package:provider/provider.dart';
import 'state/app_state.dart';
import 'shell/desktop_shell.dart';
import 'shell/mobile_shell.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await initializeDateFormatting('zh_CN');
  final state = AppState();
  await state.init();

  final desktop = !kIsWeb && (Platform.isWindows || Platform.isMacOS || Platform.isLinux);
  if (desktop) {
    await setupDesktopWindow();
  }

  runApp(
    ChangeNotifierProvider<AppState>.value(
      value: state,
      child: desktop ? const DesktopShell() : const MobileShell(),
    ),
  );
}