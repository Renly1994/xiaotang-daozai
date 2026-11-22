//! 待办本地提醒 —— 系统级定时通知。
//! 用 flutter_local_notifications（v19 起支持 Windows，全平台覆盖）。
//! 应用关闭后系统仍会到点通知；应用运行时由 AppState Ticker 额外做岛内提示。
import 'package:flutter_local_notifications/flutter_local_notifications.dart';
import 'package:timezone/data/latest_all.dart' as tzdata;
import 'package:timezone/timezone.dart' as tz;
import 'package:flutter_timezone/flutter_timezone.dart';
import 'ports.dart';

/// ReminderNotifier 的真实实现，委托给静态 NotificationService。
class SystemReminderNotifier implements ReminderNotifier {
  @override
  Future<void> init() => NotificationService.init();
  @override
  Future<bool> schedule(int id, String text, DateTime when) =>
      NotificationService.schedule(id, text, when);
  @override
  Future<void> cancel(int id) => NotificationService.cancel(id);
}

class NotificationService {
  static final FlutterLocalNotificationsPlugin _plugin = FlutterLocalNotificationsPlugin();
  static bool _initialized = false;

  static const _channelId = 'daozai_todo_reminder';
  static const _channelName = '待办提醒';

  /// 初始化插件 + 时区。失败时静默降级（提醒不可用但不影响主功能）。
  static Future<void> init() async {
    if (_initialized) return;
    try {
      tzdata.initializeTimeZones();
      final localTz = await FlutterTimezone.getLocalTimezone();
      tz.setLocalLocation(tz.getLocation(localTz));

      const android = AndroidInitializationSettings('@mipmap/ic_launcher');
      const darwin = DarwinInitializationSettings(
        requestAlertPermission: true,
        requestSoundPermission: true,
      );
      const linux = LinuxInitializationSettings(defaultActionName: '打开');
      const windows = WindowsInitializationSettings(
        appName: 'Daozai',
        appUserModelId: 'com.daozai.daozai_flutter',
        guid: 'a1b2c3d4-e5f6-7890-abcd-ef1234567890',
      );
      await _plugin.initialize(
        const InitializationSettings(
          android: android, iOS: darwin, macOS: darwin, linux: linux, windows: windows,
        ),
      );
      _initialized = true;
    } catch (_) {
      // 初始化失败：提醒功能降级，主功能不受影响。
      _initialized = false;
    }
  }

  static NotificationDetails get _details => const NotificationDetails(
        android: AndroidNotificationDetails(
          _channelId, _channelName,
          channelDescription: '待办事项到点提醒',
          importance: Importance.high, priority: Priority.high,
        ),
        iOS: DarwinNotificationDetails(),
        macOS: DarwinNotificationDetails(),
        linux: LinuxNotificationDetails(),
        windows: WindowsNotificationDetails(),
      );

  /// 安排一条待办提醒。[id] 用待办的稳定哈希，[when] 为提醒时间。
  /// 过去的时间不安排（返回 false）。
  static Future<bool> schedule(int id, String text, DateTime when) async {
    if (!_initialized) await init();
    if (!_initialized) return false;
    if (!when.isAfter(DateTime.now())) return false;
    try {
      // 锁定 19.0.0：位置参数 (id, title, body, scheduledDate, details)。
      // 注意 20.x 改为命名参数，升级时需同步修改（见 pubspec 版本锁定说明）。
      await _plugin.zonedSchedule(
        id, '待办提醒', text,
        tz.TZDateTime.from(when, tz.local),
        _details,
        androidScheduleMode: AndroidScheduleMode.exactAllowWhileIdle,
      );
      return true;
    } catch (_) {
      return false;
    }
  }

  /// 取消某条提醒（待办删除/完成/改时间时调用）。
  static Future<void> cancel(int id) async {
    if (!_initialized) return;
    try {
      await _plugin.cancel(id);
    } catch (_) {}
  }
}