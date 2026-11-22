//! Widget 交互测试 —— 待办页输入添加、删除二次确认、提醒横幅。
//! 用 fake 依赖注入 AppState，模拟真实用户交互，CI 无设备可跑。
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:daozai_flutter/state/app_state.dart';
import 'package:daozai_flutter/services/local_store.dart';
import 'package:daozai_flutter/services/ports.dart';
import 'package:daozai_flutter/ui/pages.dart';
import 'package:daozai_flutter/core/theme.dart';

class FakeStore implements TodoStore {
  List<TodoItem> saved = [];
  @override
  Future<List<TodoItem>> loadTodos() async => saved;
  @override
  Future<void> saveTodos(List<TodoItem> items) async { saved = items; }
}

class FakeNotifier implements ReminderNotifier {
  @override
  Future<void> init() async {}
  @override
  Future<bool> schedule(int id, String text, DateTime when) async => true;
  @override
  Future<void> cancel(int id) async {}
}

/// 把待办页包进测试所需的 Provider + MaterialApp + 深色主题。
Widget wrap(AppState state) {
  return ChangeNotifierProvider<AppState>.value(
    value: state,
    child: MaterialApp(
      theme: buildDaozaiTheme(),
      home: const Scaffold(body: SizedBox(width: 420, height: 200, child: TodoPage())),
    ),
  );
}

void main() {
  late AppState state;

  setUp(() {
    state = AppState(store: FakeStore(), notifier: FakeNotifier(), clock: () => DateTime(2026, 11, 22, 12));
  });
  tearDown(() => state.dispose());

  testWidgets('输入待办回车后出现在列表', (tester) async {
    await tester.pumpWidget(wrap(state));
    await tester.enterText(find.byType(TextField), '测试任务');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pump();
    expect(find.text('测试任务'), findsOneWidget);
    expect(state.todos.length, 1);
  });

  testWidgets('空列表显示占位文案', (tester) async {
    await tester.pumpWidget(wrap(state));
    expect(find.text('暂无待办'), findsOneWidget);
  });

  testWidgets('删除需二次确认（防误触）', (tester) async {
    await state.addTodo('要删的');
    await tester.pumpWidget(wrap(state));
    await tester.pump();
    // 第一次点删除图标 → 进入确认态，不真正删除。
    await tester.tap(find.byIcon(Icons.close).first);
    await tester.pump();
    expect(state.todos.length, 1);
    expect(find.byIcon(Icons.delete_forever), findsOneWidget);
    // 第二次点（确认态图标）→ 真正删除。
    await tester.tap(find.byIcon(Icons.delete_forever));
    await tester.pump();
    expect(state.todos, isEmpty);
  });

  testWidgets('勾选待办切换完成样式', (tester) async {
    await state.addTodo('任务');
    await tester.pumpWidget(wrap(state));
    await tester.pump();
    expect(find.byIcon(Icons.radio_button_unchecked), findsOneWidget);
    await tester.tap(find.byIcon(Icons.radio_button_unchecked));
    await tester.pump();
    expect(find.byIcon(Icons.check_circle), findsOneWidget);
    expect(state.todos.first.done, isTrue);
  });

  testWidgets('设置了提醒的待办显示实心铃铛', (tester) async {
    await state.addTodo('开会');
    await state.setTodoReminder(0, DateTime(2026, 11, 22, 12, 5));
    await tester.pumpWidget(wrap(state));
    await tester.pump();
    // 有提醒 → 实心铃铛 notifications_active。
    expect(find.byIcon(Icons.notifications_active), findsOneWidget);
  });

  testWidgets('到点提醒显示横幅文案', (tester) async {
    // 用可控时钟：先 12:00 设置 12:05 提醒，再推进到 12:06 触发。
    var now = DateTime(2026, 11, 22, 12, 0);
    final s = AppState(store: FakeStore(), notifier: FakeNotifier(), clock: () => now);
    await s.addTodo('开会');
    await s.setTodoReminder(0, DateTime(2026, 11, 22, 12, 5));
    await tester.pumpWidget(wrap(s));
    now = DateTime(2026, 11, 22, 12, 6);
    s.debugTick();
    await tester.pump();
    expect(find.textContaining('开会'), findsWidgets); // 横幅含待办文案
    expect(s.reminderAlert, contains('开会'));
    s.dispose();
  });
}