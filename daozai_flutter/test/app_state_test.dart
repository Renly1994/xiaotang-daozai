//! 状态机单元测试 —— 待办增删改勾选、提醒设置/触发/取消、操作锁定。
//! 用 fake store/notifier + 可控时钟，无需平台插件，CI 无设备可跑。
import 'package:flutter_test/flutter_test.dart';
import 'package:daozai_flutter/state/app_state.dart';
import 'package:daozai_flutter/services/local_store.dart';
import 'package:daozai_flutter/services/ports.dart';

/// 内存 fake 存储。
class FakeStore implements TodoStore {
  List<TodoItem> saved = [];
  int saveCount = 0;
  @override
  Future<List<TodoItem>> loadTodos() async => saved;
  @override
  Future<void> saveTodos(List<TodoItem> items) async {
    saved = items.map((e) => TodoItem(id: e.id, text: e.text, done: e.done, reminder: e.reminder)).toList();
    saveCount++;
  }
}

/// 记录调用的 fake 通知器。failSchedule 模拟系统通知不可用。
class FakeNotifier implements ReminderNotifier {
  final List<int> scheduled = [];
  final List<int> cancelled = [];
  bool initCalled = false;
  bool failSchedule = false;
  @override
  Future<void> init() async { initCalled = true; }
  @override
  Future<bool> schedule(int id, String text, DateTime when) async {
    if (failSchedule) return false;
    scheduled.add(id);
    return true;
  }
  @override
  Future<void> cancel(int id) async { cancelled.add(id); }
}

void main() {
  late FakeStore store;
  late FakeNotifier notifier;
  late DateTime fakeNow;
  late AppState state;

  setUp(() {
    store = FakeStore();
    notifier = FakeNotifier();
    fakeNow = DateTime(2026, 11, 22, 12, 0, 0);
    state = AppState(store: store, notifier: notifier, clock: () => fakeNow);
  });

  tearDown(() => state.dispose());

  group('待办增删改', () {
    test('添加待办：插入首位并持久化', () async {
      await state.addTodo('买牛奶');
      expect(state.todos.length, 1);
      expect(state.todos.first.text, '买牛奶');
      expect(store.saveCount, 1);
    });

    test('空白待办不添加', () async {
      await state.addTodo('   ');
      await state.addTodo('');
      expect(state.todos, isEmpty);
      expect(store.saveCount, 0);
    });

    test('待办文本去首尾空白', () async {
      await state.addTodo('  写报告  ');
      expect(state.todos.first.text, '写报告');
    });

    test('新待办插入首位（最新在上）', () async {
      await state.addTodo('第一');
      await state.addTodo('第二');
      expect(state.todos.first.text, '第二');
    });

    test('勾选切换完成状态', () async {
      await state.addTodo('任务');
      await state.toggleTodo(0);
      expect(state.todos.first.done, isTrue);
      await state.toggleTodo(0);
      expect(state.todos.first.done, isFalse);
    });

    test('删除待办', () async {
      await state.addTodo('A');
      await state.addTodo('B');
      await state.removeTodo(0); // 删 B（首位）
      expect(state.todos.length, 1);
      expect(state.todos.first.text, 'A');
    });
  });

  group('提醒设置与通知联动', () {
    test('设置未来提醒 → 安排系统通知', () async {
      await state.addTodo('开会');
      final when = fakeNow.add(const Duration(hours: 1));
      await state.setTodoReminder(0, when);
      expect(state.todos.first.reminder, isNotNull);
      expect(notifier.scheduled, isNotEmpty);
    });

    test('设置过去时间 → 不安排通知', () async {
      await state.addTodo('过期');
      final past = fakeNow.subtract(const Duration(hours: 1));
      await state.setTodoReminder(0, past);
      expect(notifier.scheduled, isEmpty);
      expect(state.todos.first.reminder, isNull);
      expect(state.reminderNotice, contains('晚于当前时间'));
    });

    test('清除提醒 → 取消通知', () async {
      await state.addTodo('任务');
      await state.setTodoReminder(0, fakeNow.add(const Duration(hours: 1)));
      notifier.cancelled.clear();
      await state.setTodoReminder(0, null);
      expect(state.todos.first.reminder, isNull);
      expect(notifier.cancelled, isNotEmpty);
    });

    test('完成待办 → 取消通知', () async {
      await state.addTodo('任务');
      await state.setTodoReminder(0, fakeNow.add(const Duration(hours: 1)));
      notifier.cancelled.clear();
      await state.toggleTodo(0); // 标记完成
      expect(notifier.cancelled, isNotEmpty);
    });

    test('删除待办 → 取消通知', () async {
      await state.addTodo('任务');
      await state.setTodoReminder(0, fakeNow.add(const Duration(hours: 1)));
      notifier.cancelled.clear();
      await state.removeTodo(0);
      expect(notifier.cancelled, isNotEmpty);
    });

    test('同文同提醒的两条待办通知 id 不碰撞', () async {
      final when = fakeNow.add(const Duration(hours: 1));
      await state.addTodo('重复');
      await state.addTodo('重复');
      await state.setTodoReminder(0, when);
      await state.setTodoReminder(1, when);
      expect(notifier.scheduled.length, 2);
      expect(notifier.scheduled[0], isNot(notifier.scheduled[1]));
      // 取消其中一条，只杀它自己的通知 id，不波及另一条。
      notifier.cancelled.clear();
      await state.removeTodo(0);
      expect(notifier.cancelled.single, notifier.scheduled[0]);
      expect(notifier.cancelled.single, isNot(notifier.scheduled[1]));
    });

    test('安排失败 → 留下可见提示，不静默', () async {
      notifier.failSchedule = true;
      await state.addTodo('任务');
      await state.setTodoReminder(0, fakeNow.add(const Duration(hours: 1)));
      expect(state.reminderNotice, isNotNull);
      // 恢复后再次设置 → 提示清除。
      notifier.failSchedule = false;
      await state.setTodoReminder(0, fakeNow.add(const Duration(hours: 2)));
      expect(state.reminderNotice, isNull);
    });
  });

  group('提醒到点触发（岛内提示）', () {
    test('到点触发提醒横幅并展开岛体', () async {
      await state.addTodo('喝水');
      final when = fakeNow.add(const Duration(minutes: 5));
      await state.setTodoReminder(0, when);
      expect(state.reminderAlert, isNull);
      // 推进时间到提醒点之后。
      fakeNow = when.add(const Duration(seconds: 1));
      state.debugTick();
      expect(state.reminderAlert, contains('喝水'));
      expect(state.expanded, isTrue);
    });

    test('同一提醒不重复触发', () async {
      await state.addTodo('任务');
      final when = fakeNow.add(const Duration(minutes: 5));
      await state.setTodoReminder(0, when);
      fakeNow = when.add(const Duration(seconds: 1));
      state.debugTick();
      expect(state.reminderAlert, isNotNull);
      // 清除横幅后再 tick，不应再次触发（已记录 fired）。
      state.reminderAlert = null;
      fakeNow = when.add(const Duration(seconds: 1));
      state.debugTick();
      expect(state.reminderAlert, isNull);
    });

    test('同文同时间的两条待办各自显示一次提醒', () async {
      final when = fakeNow.add(const Duration(minutes: 5));
      await state.addTodo('重复');
      await state.addTodo('重复');
      await state.setTodoReminder(0, when);
      await state.setTodoReminder(1, when);
      fakeNow = when.add(const Duration(seconds: 1));
      state.debugTick();
      expect(state.debugFiredReminderCount, 2);
    });

    test('同一秒多条待办到点 → 横幅合并显示', () async {
      final when = fakeNow.add(const Duration(minutes: 5));
      await state.addTodo('会议');
      await state.addTodo('电话');
      await state.setTodoReminder(0, when);
      await state.setTodoReminder(1, when);
      fakeNow = when.add(const Duration(seconds: 1));
      state.debugTick();
      expect(state.reminderAlert, contains('会议'));
      expect(state.reminderAlert, contains('电话'));
    });

    test('已完成待办不触发提醒', () async {
      await state.addTodo('任务');
      final when = fakeNow.add(const Duration(minutes: 5));
      await state.setTodoReminder(0, when);
      await state.toggleTodo(0); // 完成
      fakeNow = when.add(const Duration(seconds: 1));
      state.debugTick();
      expect(state.reminderAlert, isNull);
    });
  });

  group('操作锁定', () {
    test('聚焦锁定与解除', () {
      expect(state.interactionLock, isFalse);
      state.setInteractionLock(true);
      expect(state.interactionLock, isTrue);
      state.setInteractionLock(false);
      expect(state.interactionLock, isFalse);
    });

    test('切页触发定时锁定', () {
      state.setPage(IslandPage.todo);
      expect(state.interactionLock, isTrue);
    });
  });

  group('初始化', () {
    test('init 加载存储并初始化通知器', () async {
      store.saved = [TodoItem(text: '已存待办')];
      await state.init();
      expect(state.todos.length, 1);
      expect(notifier.initCalled, isTrue);
    });

    test('init 重排未来提醒（重启不丢）', () async {
      store.saved = [TodoItem(text: '未来任务', reminder: '2026-11-22 14:00')];
      await state.init();
      expect(notifier.scheduled, isNotEmpty);
    });

    test('旧数据无 id → init 后补分配且互不相同', () async {
      store.saved = [TodoItem(text: 'A'), TodoItem(text: 'B')];
      await state.init();
      expect(state.todos[0].id, isNotNull);
      expect(state.todos[1].id, isNotNull);
      expect(state.todos[0].id, isNot(state.todos[1].id));
    });
  });
}
