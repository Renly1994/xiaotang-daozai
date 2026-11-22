//! 移动 App 外壳 —— Android（完整 App 形态）。
//! 精简版：时钟 + 待办。天气/喝水/专注已移除。
import 'package:flutter/material.dart';
import '../core/theme.dart';
import '../ui/island_widget.dart';

class MobileShell extends StatelessWidget {
  const MobileShell({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'DynamicIsland',
      debugShowCheckedModeBanner: false,
      theme: buildDaozaiTheme().copyWith(
        scaffoldBackgroundColor: const Color.fromRGBO(12, 12, 14, 1),
      ),
      home: const _MobileHome(),
    );
  }
}

class _MobileHome extends StatelessWidget {
  const _MobileHome();

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        backgroundColor: Colors.transparent,
        elevation: 0,
        title: const Text('DynamicIsland', style: TextStyle(fontSize: 18, fontWeight: FontWeight.w700)),
      ),
      body: const SafeArea(
        child: Column(
          children: [
            // 顶部：常驻展开的岛体卡片（非悬浮），内含待办页。
            Padding(
              padding: EdgeInsets.symmetric(horizontal: 12, vertical: 8),
              child: IslandWidget(floating: false),
            ),
            Spacer(),
          ],
        ),
      ),
    );
  }
}
