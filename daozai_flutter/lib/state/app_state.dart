//! 应用状态 —— 精简版：时钟常驻 + 待办清单。
//! 天气/喝水/专注已按产品决策移除；消息提醒/桌宠/音乐歌词/娱乐不再纳入路线。
import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:intl/intl.dart';
import '../services/local_store.dart';

/// 当前仅待办一个功能页（保留枚举以便后续扩展 AI 额度/邮箱/日历等）。
enum IslandPage { todo }

class AppState extends ChangeNotifier {
  final LocalStore _store = LocalStore();
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
  // 由两个独立来源合成：_focusLock（输入框聚焦，持久）+ _timedLock（切页，定时）。
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

  bool _initialized = false;

  Future<void> init() async {
    if (_initialized) return;
    _initialized = true;
    todos = await _store.loadTodos();
    notifyListeners();
    _ticker = Timer.periodic(const Duration(seconds: 1), (_) => _tick());
  }

  void _tick() {
    now = DateTime.now();
    notifyListeners();
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
    todos.insert(0, TodoItem(text: t));
    await _store.saveTodos(todos);
    notifyListeners();
  }

  Future<void> toggleTodo(int i) async {
    todos[i].done = !todos[i].done;
    await _store.saveTodos(todos);
    notifyListeners();
  }

  Future<void> removeTodo(int i) async {
    todos.removeAt(i);
    await _store.saveTodos(todos);
    notifyListeners();
  }

  @override
  void dispose() {
    _ticker?.cancel();
    _lockTimer?.cancel();
    super.dispose();
  }
}