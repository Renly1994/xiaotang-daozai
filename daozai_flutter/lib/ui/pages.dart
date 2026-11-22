//! 功能页 —— 精简版：仅待办清单。
//! 天气/喝水/专注页已按产品决策移除。
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../core/theme.dart';
import '../state/app_state.dart';

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

  @override
  Widget build(BuildContext context) {
    final s = context.watch<AppState>();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const PageTitle('待办', '清单', DaozaiColors.green),
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
  }
}