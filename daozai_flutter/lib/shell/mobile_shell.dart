//! 移动 App 外壳 —— Android（完整 App 形态）。
//! 安卓没有桌面灵动岛概念：这里做成常驻展开的信息卡片 + 底部页面切换。
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../core/theme.dart';
import '../state/app_state.dart';
import '../ui/island_widget.dart';
import '../ui/pages.dart';

class MobileShell extends StatelessWidget {
  const MobileShell({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: '岛仔',
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
    final s = context.watch<AppState>();
    return Scaffold(
      appBar: AppBar(
        backgroundColor: Colors.transparent,
        elevation: 0,
        title: const Text('岛仔', style: TextStyle(fontSize: 18, fontWeight: FontWeight.w700)),
        actions: [
          IconButton(
            icon: const Icon(Icons.refresh, size: 20),
            onPressed: () => s.refreshWeather(),
          ),
        ],
      ),
      body: SafeArea(
        child: Column(
          children: [
            // 顶部：常驻展开的岛体卡片（非悬浮）。
            const Padding(
              padding: EdgeInsets.symmetric(horizontal: 12, vertical: 8),
              child: IslandWidget(floating: false),
            ),
            const SizedBox(height: 8),
            // 城市设置入口。
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 16),
              child: _CityField(),
            ),
            const Spacer(),
          ],
        ),
      ),
      bottomNavigationBar: NavigationBar(
        backgroundColor: DaozaiColors.card,
        indicatorColor: DaozaiColors.alphaBlue(0.25),
        selectedIndex: IslandPage.values.indexOf(s.page),
        onDestinationSelected: (i) => s.setPage(IslandPage.values[i]),
        destinations: const [
          NavigationDestination(icon: Icon(Icons.wb_sunny_outlined), label: '天气'),
          NavigationDestination(icon: Icon(Icons.checklist), label: '待办'),
          NavigationDestination(icon: Icon(Icons.timer_outlined), label: '专注'),
          NavigationDestination(icon: Icon(Icons.water_drop_outlined), label: '喝水'),
        ],
      ),
    );
  }
}

class _CityField extends StatefulWidget {
  @override
  State<_CityField> createState() => _CityFieldState();
}

class _CityFieldState extends State<_CityField> {
  final _ctrl = TextEditingController();
  bool _filled = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (!_filled) {
      _ctrl.text = context.read<AppState>().city;
      _filled = true;
    }
  }

  @override
  void dispose() {
    _ctrl.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final s = context.read<AppState>();
    return Row(
      children: [
        Expanded(
          child: TextField(
            controller: _ctrl,
            style: const TextStyle(fontSize: 13, color: DaozaiColors.white),
            decoration: InputDecoration(
              hintText: '城市名（中英文），回车刷新天气',
              hintStyle: const TextStyle(fontSize: 13, color: DaozaiColors.dim),
              filled: true, fillColor: DaozaiColors.track,
              contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
              border: OutlineInputBorder(borderRadius: BorderRadius.circular(10), borderSide: BorderSide.none),
            ),
            onSubmitted: (v) => s.refreshWeather(newCity: v),
          ),
        ),
        const SizedBox(width: 8),
        IconButton(
          icon: const Icon(Icons.search, color: DaozaiColors.cyan),
          onPressed: () => s.refreshWeather(newCity: _ctrl.text),
        ),
      ],
    );
  }
}