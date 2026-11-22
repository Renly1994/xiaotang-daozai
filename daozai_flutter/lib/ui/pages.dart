//! 功能页 —— 精简版：仅待办清单。
//! 天气/喝水/专注页已按产品决策移除。
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../core/dialog_room.dart';
import '../core/theme.dart';
import '../services/local_store.dart';
import '../state/app_state.dart';
import 'files_page.dart';
import 'devices_page.dart';

/// 页标题（原版双色：白字 + 彩色副标题）。
class PageTitle extends StatelessWidget {
  final String a;
  final String b;
  final Color color;
  const PageTitle(this.a, this.b, this.color, {super.key});

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Text(a, style: const TextStyle(fontSize: DaozaiSize.title, fontWeight: FontWeight.w700, color: DaozaiColors.white)),
        if (b.isNotEmpty) ...[
          const SizedBox(width: 1.5),
          Text(b, style: TextStyle(fontSize: DaozaiSize.title, fontWeight: FontWeight.w700, color: color)),
        ],
      ],
    );
  }
}

// ================= 待办 =================
class TodoPage extends StatefulWidget {
  const TodoPage({super.key});
  @override
  State<TodoPage> createState() => _TodoPageState();
}

class _TodoPageState extends State<TodoPage> {
  final _ctrl = TextEditingController();
  final _focus = FocusNode();
  AppState? _state;
  // 待确认删除的项标识（用对象引用而非索引，避免列表增删后索引错位删错对象）。
  TodoItem? _confirmDelete;

  @override
  void initState() {
    super.initState();
    _focus.addListener(_onFocusChange);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _state = context.read<AppState>();
  }

  void _onFocusChange() {
    _state?.setInteractionLock(_focus.hasFocus);
  }

  @override
  void dispose() {
    _focus.removeListener(_onFocusChange);
    _state?.setInteractionLock(false);
    _focus.dispose();
    _ctrl.dispose();
    super.dispose();
  }

  /// 设置待办提醒：选日期 + 时间；已有提醒时可清除。
  /// 用对象引用定位，await 之后重新查索引——期间列表增删不会设错对象。
  Future<void> _pickReminder(BuildContext context, AppState s, TodoItem t) async {
    // 操作锁定，避免选时间过程中岛体收起。
    s.setInteractionLock(true);
    final now = DateTime.now();
    final initial = DateTime.tryParse(t.reminder ?? '') ?? now.add(const Duration(hours: 1));
    final date = await withDialogRoom(() => showDatePicker(
      context: context,
      initialDate: initial.isBefore(now) ? now : initial,
      firstDate: now,
      lastDate: now.add(const Duration(days: 365)),
      helpText: '选择提醒日期',
    ));
    if (date == null) { s.setInteractionLock(false); return; }
    if (!context.mounted) return;
    final time = await withDialogRoom(() => showTimePicker(
      context: context,
      initialTime: TimeOfDay.fromDateTime(initial),
      helpText: '选择提醒时间',
    ));
    if (time == null) { s.setInteractionLock(false); return; }
    if (!context.mounted) return;
    final i = s.todos.indexOf(t);
    if (i < 0) { s.setInteractionLock(false); return; } // 待办已被删除
    final when = DateTime(date.year, date.month, date.day, time.hour, time.minute);
    await s.setTodoReminder(i, when);
    s.setInteractionLock(false);
  }

  @override
  Widget build(BuildContext context) {
    final s = context.watch<AppState>();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const PageTitle('待办', '清单', DaozaiColors.green),
        // 到点提醒横幅（短暂显示，让用户看到哪条待办到点了）。
        if (s.reminderAlert != null) ...[
          const SizedBox(height: 4),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
            decoration: BoxDecoration(
              color: DaozaiColors.alphaBlue(0.2),
              borderRadius: BorderRadius.circular(6),
            ),
            child: Row(
              children: [
                const Icon(Icons.notifications_active, size: 13, color: DaozaiColors.cyan),
                const SizedBox(width: 5),
                Expanded(
                  child: Text(s.reminderAlert!,
                      maxLines: 1, overflow: TextOverflow.ellipsis,
                      style: const TextStyle(fontSize: DaozaiSize.small, color: DaozaiColors.cyan)),
                ),
              ],
            ),
          ),
        ],
        // 提醒安排失败可见提示（不静默吞掉，用户知道铃铛没设上）。
        if (s.reminderNotice != null) ...[
          const SizedBox(height: 4),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
            decoration: BoxDecoration(
              color: DaozaiColors.red.withOpacity(0.15),
              borderRadius: BorderRadius.circular(6),
            ),
            child: Row(
              children: [
                const Icon(Icons.error_outline, size: 13, color: DaozaiColors.red),
                const SizedBox(width: 5),
                Expanded(
                  child: Text(s.reminderNotice!,
                      maxLines: 2, overflow: TextOverflow.ellipsis,
                      style: const TextStyle(fontSize: DaozaiSize.small, color: DaozaiColors.red)),
                ),
              ],
            ),
          ),
        ],
        const SizedBox(height: 6),
        Row(
          children: [
            Expanded(
              child: SizedBox(
                height: 26,
                child: TextField(
                  controller: _ctrl,
                  focusNode: _focus,
                  style: const TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.white),
                  decoration: InputDecoration(
                    hintText: '要做点什么？回车保存',
                    hintStyle: const TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.dim),
                    filled: true, fillColor: DaozaiColors.track,
                    contentPadding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                    border: OutlineInputBorder(borderRadius: BorderRadius.circular(7), borderSide: BorderSide.none),
                  ),
                  onSubmitted: (v) { s.addTodo(v); _ctrl.clear(); },
                ),
              ),
            ),
            const SizedBox(width: 6),
            _iconBtn(Icons.add, () { s.addTodo(_ctrl.text); _ctrl.clear(); }),
          ],
        ),
        const SizedBox(height: 6),
        Expanded(
          child: s.todos.isEmpty
              ? const Center(child: Text('暂无待办', style: TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.dim)))
              : ListView.builder(
                  padding: EdgeInsets.zero,
                  itemCount: s.todos.length,
                  itemBuilder: (_, i) {
                    final t = s.todos[i];
                    return SizedBox(
                      height: 24,
                      child: Row(
                        children: [
                          GestureDetector(
                            onTap: () => s.toggleTodo(i),
                            child: Icon(t.done ? Icons.check_circle : Icons.radio_button_unchecked,
                                size: 15, color: t.done ? DaozaiColors.green : DaozaiColors.dim),
                          ),
                          const SizedBox(width: 6),
                          Expanded(
                            child: Text(t.text,
                                maxLines: 1, overflow: TextOverflow.ellipsis,
                                style: TextStyle(fontSize: DaozaiSize.body,
                                    color: t.done ? DaozaiColors.dim : DaozaiColors.white,
                                    decoration: t.done ? TextDecoration.lineThrough : null)),
                          ),
                          // 提醒设置：点按选时间，长按清除。有提醒显示实心铃铛。
                          GestureDetector(
                            onTap: () => _pickReminder(context, s, t),
                            onLongPress: t.reminder != null
                                ? () {
                                    final i = s.todos.indexOf(t);
                                    if (i >= 0) s.setTodoReminder(i, null);
                                  }
                                : null,
                            child: Icon(
                              t.reminder != null ? Icons.notifications_active : Icons.notifications_none,
                              size: 13,
                              color: t.reminder != null ? DaozaiColors.cyan : DaozaiColors.dim,
                            ),
                          ),
                          const SizedBox(width: 5),
                          GestureDetector(
                            onTap: () {
                              if (_confirmDelete == t) {
                                s.removeTodo(i);
                                setState(() => _confirmDelete = null);
                              } else {
                                setState(() => _confirmDelete = t);
                              }
                            },
                            child: Icon(
                              _confirmDelete == t ? Icons.delete_forever : Icons.close,
                              size: 13,
                              color: _confirmDelete == t ? DaozaiColors.red : DaozaiColors.dim,
                            ),
                          ),
                        ],
                      ),
                    );
                  },
                ),
        ),
      ],
    );
  }
}

// ================= 公共控件 =================
Widget _iconBtn(IconData icon, VoidCallback onTap) {
  return GestureDetector(
    onTap: onTap,
    child: Container(
      width: 24, height: 24,
      decoration: BoxDecoration(color: DaozaiColors.btn, borderRadius: BorderRadius.circular(7)),
      child: Icon(icon, size: 14, color: DaozaiColors.gray),
    ),
  );
}

/// 按当前页返回内容。
Widget pageBody(IslandPage page) {
  switch (page) {
    case IslandPage.todo: return const TodoPage();
    case IslandPage.files: return const FilesPage();
    case IslandPage.devices: return const DevicesPage();
  }
}