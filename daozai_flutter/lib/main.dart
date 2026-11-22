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

  // 主引擎承载悬浮岛。首帧完成后再建控制台，避免子引擎插件尚未注册。
  if (desktop) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _showControlConsole();
    });
  }
}

Future<void> _showControlConsole() async {
  // desktop_multi_window 在 macOS 首帧后才完成主窗口通道注册。
  // 首次创建失败时短暂重试，避免只留下悬浮岛。
  for (var attempt = 0; attempt < 3; attempt++) {
    try {
      final windows = await WindowController.getAll();
      for (final window in windows) {
        if (window.arguments == 'control-console') {
          await window.show();
          return;
        }
      }
      await WindowController.create(
        const WindowConfiguration(
          arguments: 'control-console',
          hiddenAtLaunch: false,
        ),
      );
      return;
    } catch (e) {
      debugPrint('主控制台启动失败（第 ${attempt + 1} 次）: $e');
      await Future<void>.delayed(const Duration(milliseconds: 350));
    }
  }
}
