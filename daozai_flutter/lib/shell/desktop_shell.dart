//! 桌面悬浮外壳 —— Windows / macOS / Linux。
//! 透明、置顶、无边框窗口，岛体停在屏幕顶部居中，鼠标靠近展开。
//! 退出入口：右键岛体弹菜单「退出」；展开面板右上角关闭按钮；macOS Dock 图标右键退出。
//! 拖动：收起态按住药丸拖动窗口，位置持久化（重启恢复到上次位置）。
import 'dart:io';
import 'package:desktop_drop/desktop_drop.dart';
import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:provider/provider.dart';
import 'package:window_manager/window_manager.dart';
import '../core/dialog_room.dart';
import '../core/shelf_runtime.dart';
import '../core/theme.dart';
import '../core/quit.dart';
import '../core/window_pos.dart';
import '../state/app_state.dart';
import '../state/sync_state.dart';
import '../ui/island_widget.dart';

/// 岛体窗口常态尺寸（收起/展开面板都装得下）。
const _normalSize = Size(560, 220);

Future<void> setupDesktopWindow() async {
  await windowManager.ensureInitialized();
  const opts = WindowOptions(
    size: _normalSize,
    center: false,
    backgroundColor: Colors.transparent,
    // 保留任务栏/Dock 图标，用户可从 Dock 右键退出（不再 skipTaskbar）。
    skipTaskbar: false,
    titleBarStyle: TitleBarStyle.hidden,
    windowButtonVisibility: false,
    alwaysOnTop: true,
  );
  await windowManager.waitUntilReadyToShow(opts, () async {
    await windowManager.setHasShadow(false);
    await windowManager.setBackgroundColor(Colors.transparent);
    await windowManager.setAlwaysOnTop(true);
    // 恢复上次拖动到的位置；首次启动默认顶部居中。
    final pos = await loadWindowPosition();
    if (pos != null) {
      await windowManager.setPosition(pos);
    } else {
      await windowManager.setAlignment(Alignment.topCenter);
    }
    await windowManager.show();
    await windowManager.focus();
  });
}

class DesktopShell extends StatelessWidget {
  const DesktopShell({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'DynamicIsland',
      debugShowCheckedModeBanner: false,
      theme: buildDaozaiTheme(),
      locale: const Locale('zh'),
      supportedLocales: const [Locale('zh'), Locale('en')],
      localizationsDelegates: const [
        GlobalMaterialLocalizations.delegate,
        GlobalWidgetsLocalizations.delegate,
        GlobalCupertinoLocalizations.delegate,
      ],
      home: const _DesktopHome(),
    );
  }
}

/// 桌面岛体宿主。
///
/// 点击穿透：IslandWidget 用 window_manager.setIgnoreMouseEvents(forward:true)
/// 实现轮廓穿透——鼠标在岛体内正常交互，岛体外透明区点击落到下层窗口
/// （Windows/macOS 生效；Linux 不支持 forward，保持整窗交互）。
class _DesktopHome extends StatelessWidget {
  const _DesktopHome();

  @override
  Widget build(BuildContext context) {
    final shelf = context.read<ShelfRuntime?>();
    final sync = context.read<SyncState?>();
    final island = GestureDetector(
      onSecondaryTapDown: (details) => _showContextMenu(context, details.globalPosition),
      child: const IslandWidget(floating: true),
    );
    return Scaffold(
      backgroundColor: Colors.transparent,
      body: Align(
        alignment: Alignment.topCenter,
        child: Padding(
          padding: const EdgeInsets.only(top: DaozaiGeom.pillGap),
          // T1 拖放收纳：整窗接收（560×220 小窗误触成本低，T3 收窄到岛体轮廓）。
          // 拖拽期临时关穿透（OLE/NSDragging 需要窗口可接收），结束恢复。
          child: shelf == null
              ? island
              : DropTarget(
                  acceptedKinds: const {DropItemKind.file, DropItemKind.directory},
                  onEnter: (_) {
                    sync?.setDropHot(true);
                    if (Platform.isWindows || Platform.isMacOS) {
                      windowManager.setIgnoreMouseEvents(false);
                    }
                  },
                  onLeave: () {
                    sync?.setDropHot(false);
                  },
                  onDrop: (details) async {
                    sync?.setDropHot(false);
                    final paths = <String>[];
                    for (final item in details.items) {
                      if (item is DropFileItem && item.localPath != null) {
                        paths.add(item.localPath!);
                      } else if (item is DropDirectoryItem && item.localPath != null) {
                        paths.add(item.localPath!);
                      }
                    }
                    if (paths.isNotEmpty) {
                      await shelf.intake.intakePaths(paths);
                    }
                  },
                  child: island,
                ),
        ),
      ),
    );
  }

  void _showContextMenu(BuildContext context, Offset position) {
    final s = context.read<AppState>();
    // 菜单期间锁定岛体，防止悬停判定把岛收起、菜单跟着错位。
    s.setInteractionLock(true);
    withDialogRoom(() => showMenu<String>(
      context: context,
      position: RelativeRect.fromLTRB(position.dx, position.dy, position.dx + 1, position.dy + 1),
      color: DaozaiColors.card,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      items: [
        PopupMenuItem<String>(
          value: 'console',
          height: 36,
          child: Row(
            children: const [
              Icon(Icons.dashboard_outlined, size: 16, color: DaozaiColors.cyan),
              SizedBox(width: 8),
              Text('打开主控制台', style: TextStyle(fontSize: 13, color: DaozaiColors.white)),
            ],
          ),
        ),
        PopupMenuItem<String>(
          value: 'files',
          height: 36,
          child: Row(
            children: const [
              Icon(Icons.folder_outlined, size: 16, color: DaozaiColors.blue),
              SizedBox(width: 8),
              Text('文件置物架', style: TextStyle(fontSize: 13, color: DaozaiColors.white)),
            ],
          ),
        ),
        PopupMenuItem<String>(
          value: 'devices',
          height: 36,
          child: Row(
            children: const [
              Icon(Icons.devices, size: 16, color: DaozaiColors.softGreen),
              SizedBox(width: 8),
              Text('设备与同步', style: TextStyle(fontSize: 13, color: DaozaiColors.white)),
            ],
          ),
        ),
        PopupMenuItem<String>(
          value: 'about',
          height: 36,
          child: Row(
            children: const [
              Icon(Icons.info_outline, size: 16, color: DaozaiColors.cyan),
              SizedBox(width: 8),
              Text('关于 DynamicIsland', style: TextStyle(fontSize: 13, color: DaozaiColors.white)),
            ],
          ),
        ),
        PopupMenuItem<String>(
          value: 'quit',
          height: 36,
          child: Row(
            children: const [
              Icon(Icons.power_settings_new, size: 16, color: DaozaiColors.red),
              SizedBox(width: 8),
              Text('退出 DynamicIsland', style: TextStyle(fontSize: 13, color: DaozaiColors.white)),
            ],
          ),
        ),
      ],
    )).then((value) {
      s.setInteractionLock(false);
      if (value == 'quit') quitApp();
      if (value == 'about') _showAbout(context);
      if (value == 'console') _showControlConsole();
      if (value == 'files') {
        s.setPage(IslandPage.files);
        s.setExpanded(true);
      }
      if (value == 'devices') {
        s.setPage(IslandPage.devices);
        s.setExpanded(true);
      }
    });
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

  void _showAbout(BuildContext context) {
    final s = context.read<AppState>();
    s.setInteractionLock(true);
    withDialogRoom(() => showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: DaozaiColors.card,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
        title: const Text('DynamicIsland', style: TextStyle(color: DaozaiColors.white, fontSize: 18, fontWeight: FontWeight.w700)),
        content: const Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('跨平台桌面伙伴 · v0.1.0', style: TextStyle(color: DaozaiColors.gray, fontSize: 13)),
            SizedBox(height: 10),
            Text('时钟 · 待办清单 · 到点提醒', style: TextStyle(color: DaozaiColors.white, fontSize: 13)),
            SizedBox(height: 10),
            Text('基于开源岛仔重做的 Flutter 多平台版。\n悬停展开，按住药丸可拖动位置，右键退出。',
                style: TextStyle(color: DaozaiColors.dim, fontSize: 12, height: 1.5)),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(),
            child: const Text('知道了', style: TextStyle(color: DaozaiColors.cyan)),
          ),
        ],
      ),
    )).then((_) => s.setInteractionLock(false));
  }
}

bool get isDesktop => Platform.isWindows || Platform.isMacOS || Platform.isLinux;
