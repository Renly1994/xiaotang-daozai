//! 应用状态 —— 对应原项目 app.rs 的 App 状态机（骨架阶段精简版）。
//! 时间一律由外部 Ticker 提供，便于测试与复现。
import 'dart:async';
import 'dart:math' as math;
import 'package:flutter/foundation.dart';
import 'package:intl/intl.dart';
import '../services/weather_service.dart';
import '../services/local_store.dart';

enum IslandPage { weather, todo, focus, water }

class AppState extends ChangeNotifier {
  final WeatherService _weather = WeatherService();
  final LocalStore _store = LocalStore();
  Timer? _ticker;
  Timer? _weatherTimer;

  // 时钟
  DateTime now = DateTime.now();
  String get clock => DateFormat('HH:mm').format(now);
  String get clockSeconds => DateFormat('HH:mm:ss').format(now);
  String get dateLabel => DateFormat('M月d日 EEE', 'zh_CN').format(now);
  String get today => DateFormat('yyyy-MM-dd').format(now);

  // 页面
  IslandPage page = IslandPage.weather;
  bool expanded = false;
  // 操作锁定：输入框聚焦或刚切页时为 true，桌面岛在此期间不收起，
  // 保证用户来得及输入城市/切页/点按钮（解决悬停展开打断操作的问题）。
  // 由两个独立来源合成：_focusLock（输入框聚焦，持久）+ _timedLock（切页，定时）。
  bool _focusLock = false;
  bool _timedLock = false;
  bool get interactionLock => _focusLock || _timedLock;

  void setInteractionLock(bool v) {
    if (_focusLock == v) return;
    _focusLock = v;
    notifyListeners();
  }

  // 天气
  WeatherSnapshot? weather;
  bool weatherLoading = false;
  String? weatherError;
  String city = '';
  DateTime? _lastWeatherFetch;

  // 待办
  List<TodoItem> todos = [];

  // 喝水
  WaterState water = WaterState(day: '');

  // 专注
  int focusMinutes = 25;
  bool focusRunning = false;
  bool focusIsRest = false;
  double focusRemaining = 25 * 60;
  // 阶段结束提示（对齐原版「该休息了/该专注了」），用户据此手动开始下一阶段。
  String? focusMessage;

  bool _initialized = false;

  Future<void> init() async {
    if (_initialized) return;
    _initialized = true;
    city = await _store.loadCity();
    todos = await _store.loadTodos();
    water = await _store.loadWater(today);
    focusMinutes = await _store.loadFocusMinutes();
    focusRemaining = focusMinutes * 60.0;
    weather = await _weather.loadCache();
    notifyListeners();

    _ticker = Timer.periodic(const Duration(seconds: 1), (_) => _tick());
    _weatherTimer = Timer.periodic(const Duration(minutes: 15), (_) => refreshWeather());
    if (city.isNotEmpty) {
      refreshWeather();
    }
  }

  void _tick() {
    now = DateTime.now();
    if (focusRunning) {
      focusRemaining -= 1;
      if (focusRemaining <= 0) {
        // 阶段结束：停止并提示用户开始下一阶段（对齐原版番茄钟逻辑）。
        focusRunning = false;
        focusIsRest = !focusIsRest;
        focusRemaining = (focusIsRest ? 5 : focusMinutes) * 60.0;
        focusMessage = focusIsRest ? '专注完成，该休息一下了' : '休息结束，该专注了';
      }
    }
    notifyListeners();
  }

  void setPage(IslandPage p) {
    page = p;
    // 切页后短暂锁定展开，避免鼠标划出瞬间收起打断操作。
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

  // ---- 天气 ----
  Future<void> refreshWeather({String? newCity}) async {
    if (newCity != null) {
      final trimmed = newCity.trim();
      // 城市变化时重置节流，允许立即查询新城市（不被旧城市的 30 秒节流挡住）。
      if (trimmed != city) _lastWeatherFetch = null;
      city = trimmed;
      await _store.saveCity(city);
    }
    if (city.isEmpty) {
      weatherError = '请先在设置里填写城市';
      notifyListeners();
      return;
    }
    // 同城 30 秒内不重复请求（对齐原项目节流）。
    final ts = DateTime.now();
    if (_lastWeatherFetch != null && ts.difference(_lastWeatherFetch!).inSeconds < 30 && newCity == null) {
      return;
    }
    weatherLoading = true;
    weatherError = null;
    notifyListeners();
    try {
      final pos = await _weather.geocode(city);
      if (pos == null) {
        weatherError = '找不到城市「$city」';
      } else {
        weather = await _weather.fetch(pos);
        await _weather.saveCache(weather!);
        _lastWeatherFetch = ts;
        weatherError = null;
      }
    } catch (e) {
      weatherError = '天气更新失败，显示缓存';
    }
    weatherLoading = false;
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

  // ---- 喝水 ----
  Future<void> drink() async {
    water.drunkMl += water.servingMl;
    await _store.saveWater(water);
    notifyListeners();
  }

  Future<void> undoDrink() async {
    // 撤销只减一次饮水量，下限 0；与 drink 一样不设上限（逻辑对称）。
    water.drunkMl = math.max(0, water.drunkMl - water.servingMl);
    await _store.saveWater(water);
    notifyListeners();
  }

  Future<void> setWaterGoal(int ml) async {
    water.goalMl = ml.clamp(200, 6000);
    await _store.saveWater(water);
    notifyListeners();
  }

  Future<void> setWaterServing(int ml) async {
    water.servingMl = ml.clamp(50, 1000);
    await _store.saveWater(water);
    notifyListeners();
  }

  // ---- 专注 ----
  void toggleFocus() {
    focusRunning = !focusRunning;
    if (focusRunning) focusMessage = null;
    notifyListeners();
  }

  void resetFocus() {
    focusRunning = false;
    focusIsRest = false;
    focusRemaining = focusMinutes * 60.0;
    focusMessage = null;
    notifyListeners();
  }

  Future<void> setFocusMinutes(int m) async {
    focusMinutes = m;
    await _store.saveFocusMinutes(m);
    if (!focusRunning) focusRemaining = m * 60.0;
    notifyListeners();
  }

  String get focusLabel {
    // clamp 到非负，避免阶段切换瞬间剩余值为负时显示 "-1:-5" 这类异常。
    final s = math.max(0, focusRemaining.ceil());
    return '${(s ~/ 60).toString().padLeft(2, '0')}:${(s % 60).toString().padLeft(2, '0')}';
  }

  @override
  void dispose() {
    _ticker?.cancel();
    _weatherTimer?.cancel();
    _lockTimer?.cancel();
    _weather.dispose();
    super.dispose();
  }
}