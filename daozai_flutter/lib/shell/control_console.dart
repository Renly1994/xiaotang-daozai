//! 主控制台 —— 普通可缩放窗口。悬浮岛只承担快速查看与拖放。
import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:window_manager/window_manager.dart';
import '../core/theme.dart';

Future<void> setupControlConsoleWindow() async {
  await windowManager.ensureInitialized();
  const options = WindowOptions(
    size: Size(980, 680),
    minimumSize: Size(760, 520),
    center: true,
    backgroundColor: DaozaiColors.card,
    skipTaskbar: false,
    titleBarStyle: TitleBarStyle.normal,
    alwaysOnTop: false,
    title: 'DynamicIsland 主控制台',
  );
  await windowManager.waitUntilReadyToShow(options, () async {
    await windowManager.show();
    await windowManager.focus();
  });
}

class ControlConsoleApp extends StatelessWidget {
  const ControlConsoleApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'DynamicIsland 主控制台',
      debugShowCheckedModeBanner: false,
      theme: buildDaozaiTheme().copyWith(scaffoldBackgroundColor: DaozaiColors.card),
      locale: const Locale('zh'),
      supportedLocales: const [Locale('zh'), Locale('en')],
      localizationsDelegates: const [
        GlobalMaterialLocalizations.delegate,
        GlobalWidgetsLocalizations.delegate,
        GlobalCupertinoLocalizations.delegate,
      ],
      home: const _ControlConsole(),
    );
  }
}

class _ControlConsole extends StatefulWidget {
  const _ControlConsole();

  @override
  State<_ControlConsole> createState() => _ControlConsoleState();
}

class _ControlConsoleState extends State<_ControlConsole> {
  var _page = 0;
  var _islandEnabled = true;
  var _syncEnabled = true;
  var _wifiOnly = true;

  static const _items = [
    (Icons.dashboard_outlined, '总览'),
    (Icons.folder_outlined, '文件置物架'),
    (Icons.devices_outlined, '设备与同步'),
    (Icons.check_circle_outline, '待办与提醒'),
    (Icons.tune, '悬浮岛'),
  ];

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Row(
        children: [
          Container(
            width: 220,
            color: DaozaiColors.track,
            padding: const EdgeInsets.fromLTRB(16, 22, 16, 16),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text('DynamicIsland', style: TextStyle(fontSize: 20, fontWeight: FontWeight.w700, color: DaozaiColors.white)),
                const SizedBox(height: 4),
                const Text('主控制台', style: TextStyle(fontSize: 12, color: DaozaiColors.cyan)),
                const SizedBox(height: 24),
                ...List.generate(_items.length, (i) => _navItem(i)),
                const Spacer(),
                const Text('悬浮岛用于快速查看、提醒和拖放。', style: TextStyle(fontSize: 11, color: DaozaiColors.dim, height: 1.4)),
              ],
            ),
          ),
          Expanded(
            child: Padding(
              padding: const EdgeInsets.all(32),
              child: _content(),
            ),
          ),
        ],
      ),
    );
  }

  Widget _navItem(int i) {
    final selected = _page == i;
    final item = _items[i];
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Material(
        color: selected ? DaozaiColors.alphaBlue(0.22) : Colors.transparent,
        borderRadius: BorderRadius.circular(8),
        child: InkWell(
          borderRadius: BorderRadius.circular(8),
          onTap: () => setState(() => _page = i),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 11),
            child: Row(children: [
              Icon(item.$1, size: 18, color: selected ? DaozaiColors.cyan : DaozaiColors.gray),
              const SizedBox(width: 10),
              Text(item.$2, style: TextStyle(fontSize: 13, color: selected ? DaozaiColors.white : DaozaiColors.gray)),
            ]),
          ),
        ),
      ),
    );
  }

  Widget _content() {
    switch (_page) {
      case 0:
        return _overview();
      case 1:
        return _emptyPage('文件置物架', '拖文件到悬浮岛，或在这里管理已收纳的文件。', Icons.folder_outlined);
      case 2:
        return _devices();
      case 3:
        return _emptyPage('待办与提醒', '在这里查看、编辑和管理待办提醒。', Icons.check_circle_outline);
      case 4:
        return _islandSettings();
    }
    return const SizedBox();
  }

  Widget _overview() => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text('总览', style: TextStyle(fontSize: 28, fontWeight: FontWeight.w700)),
          const SizedBox(height: 8),
          const Text('在主控制台设置功能。悬浮岛只提供快捷操作。', style: TextStyle(color: DaozaiColors.gray)),
          const SizedBox(height: 30),
          Wrap(spacing: 16, runSpacing: 16, children: [
            _metric(Icons.folder_outlined, '文件置物架', '拖文件到岛体开始收纳'),
            _metric(Icons.devices_outlined, '设备与同步', '局域网同步等待配对'),
            _metric(Icons.check_circle_outline, '待办与提醒', '打开悬浮岛快速查看'),
          ]),
        ],
      );

  Widget _metric(IconData icon, String title, String text) => Container(
        width: 250,
        padding: const EdgeInsets.all(18),
        decoration: BoxDecoration(color: DaozaiColors.track, borderRadius: BorderRadius.circular(14)),
        child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
          Icon(icon, color: DaozaiColors.cyan),
          const SizedBox(height: 20),
          Text(title, style: const TextStyle(fontSize: 16, fontWeight: FontWeight.w600)),
          const SizedBox(height: 6),
          Text(text, style: const TextStyle(fontSize: 12, color: DaozaiColors.dim)),
        ]),
      );

  Widget _devices() => Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
        const Text('设备与同步', style: TextStyle(fontSize: 28, fontWeight: FontWeight.w700)),
        const SizedBox(height: 12),
        _switchRow('启用局域网同步', '仅在已配对设备之间同步。', _syncEnabled, (v) => setState(() => _syncEnabled = v)),
        _switchRow('仅 Wi-Fi 同步', '避免移动网络产生流量。', _wifiOnly, (v) => setState(() => _wifiOnly = v)),
        const SizedBox(height: 24),
        const Text('已配对设备', style: TextStyle(fontSize: 17, fontWeight: FontWeight.w600)),
        const SizedBox(height: 10),
        const Text('暂无设备。请在悬浮岛的设备页发起或加入配对。', style: TextStyle(color: DaozaiColors.dim)),
      ]);

  Widget _islandSettings() => Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
        const Text('悬浮岛', style: TextStyle(fontSize: 28, fontWeight: FontWeight.w700)),
        const SizedBox(height: 12),
        _switchRow('显示悬浮岛', '关闭后仍可从主控制台重新打开。', _islandEnabled, (v) => setState(() => _islandEnabled = v)),
        const SizedBox(height: 20),
        const Text('悬浮岛快捷功能', style: TextStyle(fontSize: 17, fontWeight: FontWeight.w600)),
        const SizedBox(height: 10),
        const Text('时间、提醒、拖放文件和主控制台入口。', style: TextStyle(color: DaozaiColors.dim)),
      ]);

  Widget _switchRow(String title, String subtitle, bool value, ValueChanged<bool> onChanged) => Container(
        margin: const EdgeInsets.only(bottom: 10),
        padding: const EdgeInsets.all(16),
        decoration: BoxDecoration(color: DaozaiColors.track, borderRadius: BorderRadius.circular(12)),
        child: Row(children: [
          Expanded(child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(title, style: const TextStyle(fontSize: 14, fontWeight: FontWeight.w600)),
            const SizedBox(height: 4),
            Text(subtitle, style: const TextStyle(fontSize: 12, color: DaozaiColors.dim)),
          ])),
          Switch(value: value, activeThumbColor: DaozaiColors.green, onChanged: onChanged),
        ]),
      );

  Widget _emptyPage(String title, String text, IconData icon) => Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Text(title, style: const TextStyle(fontSize: 28, fontWeight: FontWeight.w700)),
        const SizedBox(height: 28),
        Expanded(child: Center(child: Column(mainAxisSize: MainAxisSize.min, children: [
          Icon(icon, size: 48, color: DaozaiColors.dim),
          const SizedBox(height: 14),
          Text(text, style: const TextStyle(color: DaozaiColors.gray)),
        ]))),
      ]);
}
