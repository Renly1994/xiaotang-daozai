//! 桌面岛体回归测试 —— 透明窗药丸居中、macOS 原生鼠标转发驱动的悬停/穿透、位置持久化。
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:dynamic_island/core/theme.dart';
import 'package:dynamic_island/core/window_pos.dart';
import 'package:dynamic_island/services/local_store.dart';
import 'package:dynamic_island/services/ports.dart';
import 'package:dynamic_island/state/app_state.dart';
import 'package:dynamic_island/ui/island_shape.dart';
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
  late List<MethodCall> wmCalls;
  late AppState state;

  setUp(() {
    wmCalls = [];
    state = AppState(store: FakeStore(), notifier: FakeNotifier());
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(wm, (call) async {
      wmCalls.add(call);
      if (call.method == 'getBounds') {
        return {'x': 754.0, 'y': 119.0, 'width': 560.0, 'height': 220.0};
      }
      return null;
    });
  });

  tearDown(() {
    state.dispose();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(wm, null);
  });

  Widget wrap() => ChangeNotifierProvider<AppState>.value(
        value: state,
        child: MaterialApp(
          theme: buildDaozaiTheme(),
          home: const MediaQuery(
            data: MediaQueryData(size: Size(560, 220)),
            child: Scaffold(body: IslandWidget(floating: true)),
          ),
        ),
      );

  Future<void> nativeMove(WidgetTester tester, double x, double y) async {
    final data = const StandardMethodCodec()
        .encodeMethodCall(MethodCall('mouseLocal', [x, y]));
    await tester.binding.defaultBinaryMessenger
        .handlePlatformMessage('dynamicisland/mouse', data, (_) {});
  }

  test('收起药丸在画布内水平居中，展开时偏移归零', () {
    const openW = DaozaiGeom.panelW + DaozaiGeom.flare * 2 + 24;
    expect(islandBodyOffsetX(openW, DaozaiGeom.barW),
        closeTo((openW - DaozaiGeom.barW) / 2, 1e-9));
    expect(islandBodyOffsetX(openW, openW), 0);
  });

  testWidgets('启动即穿透；原生鼠标移入岛体恢复交互并悬停展开', (tester) async {
    await tester.pumpWidget(wrap());
    expect(wmCalls.last.method, 'setIgnoreMouseEvents');
    expect(wmCalls.last.arguments['ignore'], isTrue);

    await nativeMove(tester, 240, 15);
    expect(wmCalls.last.arguments['ignore'], isFalse);
    await tester.pump(const Duration(milliseconds: 50));
    await tester.pump(const Duration(milliseconds: 250));
    expect(state.expanded, isTrue);
  });

  testWidgets('原生鼠标移出岛体恢复穿透并收起', (tester) async {
    await tester.pumpWidget(wrap());
    await nativeMove(tester, 240, 15);
    await tester.pump(const Duration(milliseconds: 50));
    await tester.pump(const Duration(milliseconds: 250));
    expect(state.expanded, isTrue);

    await nativeMove(tester, 10, 200);
    expect(wmCalls.last.arguments['ignore'], isTrue);
    await tester.pump(const Duration(milliseconds: 50));
    await tester.pump(const Duration(milliseconds: 300));
    expect(state.expanded, isFalse);
  });

  testWidgets('穿透态点击药丸仍可展开（点击不被吞）', (tester) async {
    await tester.pumpWidget(wrap());
    await tester.tapAt(const Offset(240, 15));
    await tester.pump(const Duration(milliseconds: 50));
    await tester.pump(const Duration(milliseconds: 250));
    expect(state.expanded, isTrue);
  });

  testWidgets('拖动结束保存的位置重启后可读回', (tester) async {
    SharedPreferences.setMockInitialValues({});
    await saveWindowPosition();
    final pos = await loadWindowPosition();
    expect(pos, const Offset(754, 119));
  });
}