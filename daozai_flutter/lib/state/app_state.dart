//! 应用状态 —— 精简版：时钟常驻 + 待办清单（含本地提醒）。
//! 天气/喝水/专注已按产品决策移除；消息提醒/桌宠/音乐歌词/娱乐不再纳入路线。
import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:intl/intl.dart';
import '../services/local_store.dart';
import '../services/notification_service.dart';
import '../services/ports.dart';

/// 当前仅待办一个功能页（保留枚举以便后续扩展 AI 额度/邮箱/日历等）。
enum IslandPage { todo }

class AppState extends ChangeNotifier {
  /// 依赖注入：测试可传入 fake store/notifier 与可控 clock。
  AppState({TodoStore? store, ReminderNotifier? notifier, Clock? clock})
      : _store = store ?? LocalStore(),
        _notifier = notifier ?? SystemReminderNotifier(),
        _clock = clock ?? defaultClock;

  final TodoStore _store;
  final ReminderNotifier _notifier;
  final Clock _clock;
  Timer? _ticker;

  // 时钟
  DateTime now = DateTime.now();
  String get clock => DateFormat('HH:mm').format(now);
  String get clockSeconds => DateFormat('HH:mm:ss').format(now);
  String get dateLabel => DateFormat('M月d日 EEE', 'zh_CN').format(now);
  String get today => DateFormat('yyyy-MM-dd').format(now);

  // 页面
  IslandPage page = IslandPage.todo;
  bool expanded = false;
  // 操作锁定：输入框聚焦或刚切页时为 true，桌面岛在此期间不收起。
  bool _focusLock = false;
  bool _timedLock = false;
  bool get interactionLock => _focusLock || _timedLock;

  void setInteractionLock(bool v) {
    if (_focusLock == v) return;
    _focusLock = v;
    notifyListeners();
  }

  // 待办
  List<TodoItem> todos = [];
  // 到点提醒的岛内提示文案（短暂显示后自动清除）。
  String? reminderAlert;
  // 提醒安排失败的可见提示（静默失败会让用户以为设好了，必须说出来）。
  String? reminderNotice;
  Timer? _alertTimer;
  // 已触发过提醒的待办标识，防止同一秒重复触发。
  final Set<String> _firedReminders = {};

  bool _initialized = false;

  Future<void> init() async {
    if (_initialized) return;
    _initialized = true;
    todos = await _store.loadTodos();
    // 不变量：每条待办都有稳定 id（旧数据/任意 store 实现都补齐）。
    var next = todos.fold<int>(0, (m, t) => (t.id ?? 0) > m ? t.id! : m) + 1;
    for (final t in todos) {
      t.id ??= next++;
    }
    await _notifier.init();
    // 启动时为所有未完成的未来提醒重新安排系统通知（应用重启后不丢提醒）。
    for (final t in todos) {
      final when = _parseReminder(t.reminder);
      if (!t.done && when != null && when.isAfter(_clock())) {
        await _scheduleChecked(t, when);
      }
    }
    notifyListeners();
    _ticker = Timer.periodic(const Duration(seconds: 1), (_) => _tick());
  }

  void _tick() {
    now = _clock();
    _checkReminders();
    notifyListeners();
  }

  /// 测试专用：手动驱动一次 tick（用注入的 clock 推进时间，验证提醒触发）。
  @visibleForTesting
  void debugTick() => _tick();

  /// 检测是否有待办到点：岛内弹提示 + 展开岛体让用户看到。
  void _checkReminders() {
    for (final t in todos) {
      if (t.done) continue;
      final when = _parseReminder(t.reminder);
      if (when == null) continue;
      final key = '${t.text}@${t.reminder}';
      // 到点（误差 1 秒内）且未触发过 → 提示。
      if (!now.isBefore(when) && now.difference(when).inSeconds < 2 && !_firedReminders.contains(key)) {
        _firedReminders.add(key);
        _showAlert('待办：${t.text}');
      }
    }
  }

  void _showAlert(String msg) {
    reminderAlert = msg;
    expanded = true; // 展开岛体让用户看到提醒
    _alertTimer?.cancel();
    _alertTimer = Timer(const Duration(seconds: 8), () {
      reminderAlert = null;
      notifyListeners();
    });
    notifyListeners();
  }

  DateTime? _parseReminder(String? s) {
    if (s == null || s.isEmpty) return null;
    return DateTime.tryParse(s);
  }

  /// 待办稳定 ID（用于系统通知的取消/重排）。
  /// 用持久化 id 而非文本哈希：同文同提醒的两条待办不再共享通知。
  int _todoId(TodoItem t) {
    t.id ??= _nextId();
    return t.id!;
  }

  int _nextId() =>
      todos.fold<int>(0, (m, t) => (t.id ?? 0) > m ? t.id! : m) + 1;

  /// 安排通知并如实反馈结果：失败时留下可见提示，成功则清除旧提示。
  Future<void> _scheduleChecked(TodoItem t, DateTime when) async {
    final ok = await _notifier.schedule(_todoId(t), t.text, when);
    reminderNotice = ok ? null : '提醒安排失败：系统通知不可用或被限制';
  }

  void setPage(IslandPage p) {
    page = p;
    _lockFor(const Duration(milliseconds: 800));
    notifyListeners();
  }

  Timer? _lockTimer;
  void _lockFor(Duration d) {
    _timedLock = true;
    _lockTimer?.cancel();
    _lockTimer = Timer(d, () {
      _timedLock = false;
      notifyListeners();
    });
    notifyListeners();
  }

  void setExpanded(bool v) {
    expanded = v;
    notifyListeners();
  }

  // ---- 待办 ----
  Future<void> addTodo(String text) async {
    final t = text.trim();
    if (t.isEmpty) return;
    todos.insert(0, TodoItem(text: t, id: _nextId()));
    await _store.saveTodos(todos);
    notifyListeners();
  }

  Future<void> toggleTodo(int i) async {
    final t = todos[i];
    t.done = !t.done;
    // 完成 → 取消系统通知；取消完成 → 若提醒仍在未来则重排。
    if (t.done) {
      await _notifier.cancel(_todoId(t));
    } else {
      final when = _parseReminder(t.reminder);
      if (when != null && when.isAfter(_clock())) {
        await _scheduleChecked(t, when);
      }
    }
    await _store.saveTodos(todos);
    notifyListeners();
  }

  Future<void> removeTodo(int i) async {
    final t = todos[i];
    await _notifier.cancel(_todoId(t));
    _firedReminders.removeWhere((k) => k.startsWith('${t.text}@'));
    todos.removeAt(i);
    await _store.saveTodos(todos);
    notifyListeners();
  }

  /// 设置/清除某条待办的提醒时间。[when] 为 null 表示清除提醒。
  Future<void> setTodoReminder(int i, DateTime? when) async {
    final t = todos[i];
    await _notifier.cancel(_todoId(t));
    t.reminder = when == null ? null : DateFormat('yyyy-MM-dd HH:mm').format(when);
    _firedReminders.removeWhere((k) => k.startsWith('${t.text}@'));
    if (when != null && !t.done && when.isAfter(_clock())) {
      await _scheduleChecked(t, when);
    }
    await _store.saveTodos(todos);
    notifyListeners();
  }

  @override
  void dispose() {
    _ticker?.cancel();
    _lockTimer?.cancel();
    _alertTimer?.cancel();
    super.dispose();
  }
}