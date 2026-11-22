//! 本地存储 —— 待办清单。
//! 天气/喝水/专注存储已按产品决策移除。
//! 用 shared_preferences（JSON）；正式版可换 sqlite/文件以对齐原格式。
import 'dart:convert';
import 'package:shared_preferences/shared_preferences.dart';
import 'ports.dart';

class TodoItem {
  String text;
  bool done;
  String? reminder; // 'YYYY-MM-DD HH:MM' 或 null
  TodoItem({required this.text, this.done = false, this.reminder});

  Map<String, dynamic> toJson() => {'text': text, 'done': done, 'reminder': reminder};
  factory TodoItem.fromJson(Map<String, dynamic> j) => TodoItem(
        text: j['text'] as String? ?? '',
        done: j['done'] as bool? ?? false,
        reminder: j['reminder'] as String?,
      );
}

class LocalStore implements TodoStore {
  static const _todoKey = 'daozai.todos';

  @override
  Future<List<TodoItem>> loadTodos() async {
    final prefs = await SharedPreferences.getInstance();
    final raw = prefs.getString(_todoKey);
    if (raw == null) return [];
    try {
      return (jsonDecode(raw) as List)
          .map((e) => TodoItem.fromJson(e as Map<String, dynamic>))
          .toList();
    } catch (_) {
      return [];
    }
  }

  @override
  Future<void> saveTodos(List<TodoItem> items) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_todoKey, jsonEncode(items.map((e) => e.toJson()).toList()));
  }
}