//! 桌面悬浮外壳 —— Windows / macOS / Linux。
//! 透明、置顶、无边框窗口，岛体停在屏幕顶部居中，鼠标靠近展开。
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';
import '../core/theme.dart';
import '../ui/island_widget.dart';

Future<void> setupDesktopWindow() async {
  await windowManager.ensureInitialized();
  const opts = WindowOptions(
    size: Size(560, 220),
    center: false,
    backgroundColor: Colors.transparent,
    skipTaskbar: true,
    titleBarStyle: TitleBarStyle.hidden,
    alwaysOnTop: true,
  );
  await windowManager.waitUntilReadyToShow(opts, () async {
    await windowManager.setAsFrameless();
    await windowManager.setBackgroundColor(Colors.transparent);
    await windowManager.setAlwaysOnTop(true);
    await windowManager.setSkipTaskbar(true);
    await windowManager.setAlignment(Alignment.topCenter);
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
/// 说明：原版 native.rs 用「命中区 + HTTRANSPARENT」做到只有岛体外点击穿透。
/// window_manager 暂无逐区域穿透 API，骨架阶段保持整窗接收鼠标，确保药丸可被
/// 悬停/点击唤醒；窗口已收窄到岛体尺寸，遮挡范围有限。精确命中穿透列为后续工作
/// （Windows 侧可经 Win32 SetWindowRgn / WM_NCHITTEST 通道实现）。
class _DesktopHome extends StatelessWidget {
  const _DesktopHome();

  @override
  Widget build(BuildContext context) {
    return const Scaffold(
      backgroundColor: Colors.transparent,
      body: Align(
        alignment: Alignment.topCenter,
        child: Padding(
          padding: EdgeInsets.only(top: DaozaiGeom.pillGap),
          child: IslandWidget(floating: true),
        ),
      ),
    );
  }
}

bool get isDesktop => Platform.isWindows || Platform.isMacOS || Platform.isLinux;