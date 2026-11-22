//! 本地存储 —— 待办清单。
//! 天气/喝水/专注存储已按产品决策移除。
//! 用 shared_preferences（JSON）；正式版可换 sqlite/文件以对齐原格式。
import 'dart:convert';
import 'dart:io' show Platform;
import 'package:home_widget/home_widget.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'ports.dart';

class TodoItem {
  /// 稳定 ID：通知取消/重排用它，不用文本哈希（同文同提醒会碰撞）。
  /// 旧数据无 id 时由 loadTodos 补分配。
  int? id;
  String text;
  bool done;
  String? reminder; // 'YYYY-MM-DD HH:MM' 或 null
  TodoItem({required this.text, this.done = false, this.reminder, this.id});

  Map<String, dynamic> toJson() => {'id': id, 'text': text, 'done': done, 'reminder': reminder};
  factory TodoItem.fromJson(Map<String, dynamic> j) => TodoItem(
        id: (j['id'] as num?)?.toInt(),
        text: j['text'] as String? ?? '',
        done: j['done'] as bool? ?? false,
        reminder: j['reminder'] as String?,
      );
}

class LocalStore implements TodoStore {
  static const _todoKey = 'daozai.todos';
  // CI 固定 org=com.daozai、project=daozai_flutter，Provider 全名据此确定。
  static const _widgetProvider = 'com.daozai.daozai_flutter.widget.DaozaiWidgetProvider';

  @override
  Future<List<TodoItem>> loadTodos() async {
    final prefs = await SharedPreferences.getInstance();
    final raw = prefs.getString(_todoKey);
    if (raw == null) return [];
    try {
      final items = (jsonDecode(raw) as List)
          .map((e) => TodoItem.fromJson(e as Map<String, dynamic>))
          .toList();
      // 旧数据迁移：补分配稳定 id（取现有最大值递增）。
      var next = items.fold<int>(0, (m, t) => (t.id ?? 0) > m ? t.id! : m) + 1;
      for (final t in items) {
        t.id ??= next++;
      }
      return items;
    } catch (_) {
      return [];
    }
  }

  @override
  Future<void> saveTodos(List<TodoItem> items) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_todoKey, jsonEncode(items.map((e) => e.toJson()).toList()));
    // 待办变化即推送小组件刷新计数（时钟由 TextClock 自走，不依赖刷新）。
    if (Platform.isAndroid) _pushWidget();
  }

  Future<void> _pushWidget() async {
    try {
      await HomeWidget.updateWidget(qualifiedAndroidName: _widgetProvider);
    } catch (_) {
      // 小组件刷新失败不影响待办本身。
    }
  }
}