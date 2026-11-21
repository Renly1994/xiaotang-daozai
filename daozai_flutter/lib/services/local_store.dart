//! 本地存储 —— 待办 / 喝水 / 设置，复刻原项目 model.rs 的数据模型。
//! 骨架阶段用 shared_preferences（JSON）；正式版可换 sqlite/文件以对齐原格式。
import 'dart:convert';
import 'package:shared_preferences/shared_preferences.dart';

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

class WaterState {
  int goalMl;
  int servingMl;
  int drunkMl;
  String day; // 'YYYY-MM-DD'
  WaterState({this.goalMl = 2000, this.servingMl = 250, this.drunkMl = 0, required this.day});

  double get percent => goalMl <= 0 ? 0 : (drunkMl / goalMl * 100).clamp(0, 100);

  Map<String, dynamic> toJson() =>
      {'goal': goalMl, 'serving': servingMl, 'drunk': drunkMl, 'day': day};
  factory WaterState.fromJson(Map<String, dynamic> j) => WaterState(
        goalMl: (j['goal'] as num?)?.toInt() ?? 2000,
        servingMl: (j['serving'] as num?)?.toInt() ?? 250,
        drunkMl: (j['drunk'] as num?)?.toInt() ?? 0,
        day: j['day'] as String? ?? '',
      );
}

class LocalStore {
  static const _todoKey = 'daozai.todos';
  static const _waterKey = 'daozai.water';
  static const _cityKey = 'daozai.weather.city';
  static const _focusKey = 'daozai.focus.minutes';

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

  Future<void> saveTodos(List<TodoItem> items) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_todoKey, jsonEncode(items.map((e) => e.toJson()).toList()));
  }

  Future<WaterState> loadWater(String today) async {
    final prefs = await SharedPreferences.getInstance();
    final raw = prefs.getString(_waterKey);
    if (raw != null) {
      try {
        final w = WaterState.fromJson(jsonDecode(raw) as Map<String, dynamic>);
        if (w.day == today) return w;
        // 跨天清零，保留目标与单次量。
        return WaterState(goalMl: w.goalMl, servingMl: w.servingMl, day: today);
      } catch (_) {}
    }
    return WaterState(day: today);
  }

  Future<void> saveWater(WaterState w) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_waterKey, jsonEncode(w.toJson()));
  }

  Future<String> loadCity() async {
    final prefs = await SharedPreferences.getInstance();
    return prefs.getString(_cityKey) ?? '';
  }

  Future<void> saveCity(String city) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_cityKey, city);
  }

  Future<int> loadFocusMinutes() async {
    final prefs = await SharedPreferences.getInstance();
    return prefs.getInt(_focusKey) ?? 25;
  }

  Future<void> saveFocusMinutes(int m) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setInt(_focusKey, m);
  }
}