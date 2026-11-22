//! 可测试性抽象 —— 存储与通知接口，便于单元测试注入 fake。
//! 真实实现见 local_store.dart / notification_service.dart。
import 'local_store.dart';

/// 待办存储抽象。
abstract class TodoStore {
  Future<List<TodoItem>> loadTodos();
  Future<void> saveTodos(List<TodoItem> items);
}

/// 系统通知抽象。
abstract class ReminderNotifier {
  Future<void> init();
  Future<bool> schedule(int id, String text, DateTime when);
  Future<void> cancel(int id);
}

/// 时钟抽象：测试可注入固定/可控时间，验证提醒触发等时间相关逻辑。
typedef Clock = DateTime Function();
DateTime defaultClock() => DateTime.now();