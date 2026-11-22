//! DynamicIsland 入口。
//! 桌面：透明置顶悬浮岛 + T1 置物架（拖放收纳/设备同步）。
import 'dart:io' show Platform;
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:intl/date_symbol_data_local.dart';
import 'package:provider/provider.dart';
import 'core/shelf_runtime.dart';
import 'state/app_state.dart';
import 'state/sync_state.dart';
import 'shell/desktop_shell.dart';
import 'shell/mobile_shell.dart';
import 'shell/control_console.dart';

Future<void> main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();
  await initializeDateFormatting('zh_CN');
  final desktop = !kIsWeb && (Platform.isWindows || Platform.isMacOS || Platform.isLinux);

  // 子窗口只显示主控制台。它不能再次创建岛体或监听同步端口。
  if (desktop) {
    final controller = await WindowController.fromCurrentEngine();
    if (controller.arguments == 'control-console') {
      await setupControlConsoleWindow();
      runApp(const ControlConsoleApp());
      return;
    }
  }

  final state = AppState();
  await state.init();

  if (desktop) {
    await setupDesktopWindow();
  }

  // T1 置物架运行时（桌面端；安卓 T2 接入）。失败降级：岛体主功能不受影响。
  ShelfRuntime? shelf;
  if (desktop) {
    try {
      shelf = await ShelfRuntime.create();
    } catch (e) {
      debugPrint('置物架初始化失败（主功能不受影响）: $e');
    }
  }

  runApp(
    MultiProvider(
      providers: [
        ChangeNotifierProvider<AppState>.value(value: state),
        if (shelf != null) ChangeNotifierProvider<SyncState>.value(value: shelf.state),
        if (shelf != null) Provider<ShelfRuntime>.value(value: shelf),
      ],
      child: desktop ? const DesktopShell() : const MobileShell(),
    ),
  );

  // 主引擎承载悬浮岛。控制台是独立普通窗口，二者同时启动。
  if (desktop) {
    await _showControlConsole();
  }
}

Future<void> _showControlConsole() async {
  final windows = await WindowController.getAll();
  for (final window in windows) {
    if (window.arguments == 'control-console') {
      await window.show();
      return;
    }
  }
  final console = await WindowController.create(
    const WindowConfiguration(arguments: 'control-console', hiddenAtLaunch: true),
  );
  await console.show();
}
