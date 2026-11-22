//! 桌面悬浮外壳 —— Windows / macOS / Linux。
//! 透明、置顶、无边框窗口，岛体停在屏幕顶部居中，鼠标靠近展开。
//! 退出入口：右键岛体弹菜单「退出」；展开面板右上角关闭按钮；macOS Dock 图标右键退出。
//! 拖动：收起态按住药丸拖动窗口，位置持久化（重启恢复到上次位置）。
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';
import '../core/theme.dart';
import '../core/quit.dart';
import '../core/window_pos.dart';
import '../ui/island_widget.dart';

Future<void> setupDesktopWindow() async {
  await windowManager.ensureInitialized();
  const opts = WindowOptions(
    size: Size(560, 220),
    center: false,
    backgroundColor: Colors.transparent,
    // 保留任务栏/Dock 图标，用户可从 Dock 右键退出（不再 skipTaskbar）。
    skipTaskbar: false,
    titleBarStyle: TitleBarStyle.hidden,
    alwaysOnTop: true,
  );
  await windowManager.waitUntilReadyToShow(opts, () async {
    await windowManager.setAsFrameless();
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
      title: 'Daozai',
      debugShowCheckedModeBanner: false,
      theme: buildDaozaiTheme(),
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
    return Scaffold(
      backgroundColor: Colors.transparent,
      body: Align(
        alignment: Alignment.topCenter,
        child: Padding(
          padding: const EdgeInsets.only(top: DaozaiGeom.pillGap),
          // 右键岛体弹出菜单，提供「退出」入口（悬浮窗无标题栏关闭按钮）。
          child: GestureDetector(
            onSecondaryTapDown: (details) => _showContextMenu(context, details.globalPosition),
            child: const IslandWidget(floating: true),
          ),
        ),
      ),
    );
  }

  void _showContextMenu(BuildContext context, Offset position) {
    showMenu<String>(
      context: context,
      position: RelativeRect.fromLTRB(position.dx, position.dy, position.dx + 1, position.dy + 1),
      color: DaozaiColors.card,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      items: [
        PopupMenuItem<String>(
          value: 'about',
          height: 36,
          child: Row(
            children: const [
              Icon(Icons.info_outline, size: 16, color: DaozaiColors.cyan),
              SizedBox(width: 8),
              Text('关于 Daozai', style: TextStyle(fontSize: 13, color: DaozaiColors.white)),
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
              Text('退出 Daozai', style: TextStyle(fontSize: 13, color: DaozaiColors.white)),
            ],
          ),
        ),
      ],
    ).then((value) {
      if (value == 'quit') quitApp();
      if (value == 'about') _showAbout(context);
    });
  }

  void _showAbout(BuildContext context) {
    showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: DaozaiColors.card,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
        title: const Text('Daozai', style: TextStyle(color: DaozaiColors.white, fontSize: 18, fontWeight: FontWeight.w700)),
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
    );
  }
}

bool get isDesktop => Platform.isWindows || Platform.isMacOS || Platform.isLinux;