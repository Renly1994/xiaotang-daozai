//! 投放态回归测试 —— dropHot 驱动岛体强制展开 + 高亮提示；配对锁定同理（验收 5）。
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:dynamic_island/core/theme.dart';
import 'package:dynamic_island/services/local_store.dart';
import 'package:dynamic_island/services/ports.dart';
import 'package:dynamic_island/state/app_state.dart';
import 'package:dynamic_island/state/sync_state.dart';
import 'package:dynamic_island/ui/island_widget.dart';

class FakeStore implements TodoStore {
  @override
  Future<List<TodoItem>> loadTodos() async => [];
  @override
  Future<void> saveTodos(List<TodoItem> items) async {}
}

class FakeNotifier implements ReminderNotifier {
  @override
  Future<void> init() async {}
  @override
  Future<bool> schedule(int id, String text, DateTime when) async => true;
  @override
  Future<void> cancel(int id) async {}
}

void main() {
  const wm = MethodChannel('window_manager');
  late AppState state;
  late SyncState sync;

  setUp(() {
    state = AppState(store: FakeStore(), notifier: FakeNotifier());
    sync = SyncState();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(wm, (call) async {
      if (call.method == 'getBounds') {
        return {'x': 0.0, 'y': 0.0, 'width': 560.0, 'height': 220.0};
      }
      return null;
    });
  });

  tearDown(() {
    state.dispose();
    sync.dispose();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(wm, null);
  });

  Widget wrap() => MultiProvider(
        providers: [
          ChangeNotifierProvider<AppState>.value(value: state),
          ChangeNotifierProvider<SyncState>.value(value: sync),
        ],
        child: MaterialApp(
          theme: buildDaozaiTheme(),
          home: const MediaQuery(
            data: MediaQueryData(size: Size(560, 220)),
            child: Scaffold(body: IslandWidget(floating: true)),
          ),
        ),
      );

  testWidgets('拖入投放态：收起岛体强制展开并显示「松手收纳」', (tester) async {
    await tester.pumpWidget(wrap());
    await tester.pump(const Duration(milliseconds: 100));
    expect(state.expanded, isFalse);
    sync.setDropHot(true);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('松手收纳'), findsOneWidget);
    sync.setDropHot(false);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('松手收纳'), findsNothing);
  });

  testWidgets('配对锁定期间岛体保持展开', (tester) async {
    await tester.pumpWidget(wrap());
    sync.setInteractionLockForPairing(true);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    // 锁定中：即使无悬停也不收起（面板内容可见）。
    expect(find.byType(IslandWidget), findsOneWidget);
    sync.setInteractionLockForPairing(false);
    await tester.pump(const Duration(milliseconds: 400));
  });

  testWidgets('底部导航有足够触控区域，点击可切换到文件页', (tester) async {
    state.setExpanded(true);
    sync.setInteractionLockForPairing(true);
    await tester.pumpWidget(wrap());
    // 弹簧每帧最多推进 100ms；连续 pump 才会抵达可交互阈值。
    for (var i = 0; i < 10; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    final fileDot = find.byKey(const ValueKey('island-page-files'));
    expect(fileDot, findsOneWidget);
    await tester.tap(fileDot);
    await tester.pump(const Duration(milliseconds: 50));
    expect(state.page, IslandPage.files);
    sync.setInteractionLockForPairing(false);
    await tester.pump(const Duration(seconds: 1));
  });
}
