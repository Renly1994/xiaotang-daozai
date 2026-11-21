//! 四个功能页 —— 对应原项目 pages.rs / weather_ui.rs 等（骨架精简版）。
//! 面板局部坐标 419×140，文字基线对齐原版。
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../core/theme.dart';
import '../core/weather_code.dart';
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

// ================= 天气 =================
class WeatherPage extends StatelessWidget {
  const WeatherPage({super.key});

  @override
  Widget build(BuildContext context) {
    final s = context.watch<AppState>();
    final w = s.weather;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            const PageTitle('当地', '天气', DaozaiColors.cyan),
            const Spacer(),
            if (s.weatherLoading)
              const SizedBox(width: 12, height: 12, child: CircularProgressIndicator(strokeWidth: 1.5, color: DaozaiColors.cyan))
            else
              _iconBtn(Icons.refresh, () => s.refreshWeather()),
          ],
        ),
        const SizedBox(height: 6),
        if (w == null)
          Expanded(child: _WeatherEmpty(error: s.weatherError))
        else ...[
          Row(
            crossAxisAlignment: CrossAxisAlignment.center,
            children: [
              Text(weatherEmoji(w.kind), style: const TextStyle(fontSize: 30)),
              const SizedBox(width: 8),
              Text('${w.temp.round()}°', style: const TextStyle(fontSize: DaozaiSize.hero, fontWeight: FontWeight.w700, color: DaozaiColors.white)),
              const SizedBox(width: 8),
              Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(w.place, style: const TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.white)),
                  Text('${w.describe} · 体感 ${w.feelsLike.round()}°', style: const TextStyle(fontSize: DaozaiSize.small, color: DaozaiColors.gray)),
                ],
              ),
              const Spacer(),
              Column(
                crossAxisAlignment: CrossAxisAlignment.end,
                children: [
                  Text('湿度 ${w.humidity}%', style: const TextStyle(fontSize: DaozaiSize.small, color: DaozaiColors.gray)),
                  if (w.sunrise != null) Text('日出 ${w.sunrise} · 日落 ${w.sunset}', style: const TextStyle(fontSize: DaozaiSize.tiny, color: DaozaiColors.dim)),
                ],
              ),
            ],
          ),
          const SizedBox(height: 6),
          Expanded(
            child: Row(
              children: w.days.take(7).map((d) => Expanded(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    Text(d.label, style: const TextStyle(fontSize: DaozaiSize.tiny, color: DaozaiColors.gray)),
                    const SizedBox(height: 2),
                    Text(weatherEmoji(weatherKind(d.code, true)), style: const TextStyle(fontSize: 13)),
                    const SizedBox(height: 2),
                    Text('${d.high.round()}°', style: const TextStyle(fontSize: DaozaiSize.small, color: DaozaiColors.white)),
                    Text('${d.low.round()}°', style: const TextStyle(fontSize: DaozaiSize.tiny, color: DaozaiColors.dim)),
                  ],
                ),
              )).toList(),
            ),
          ),
        ],
      ],
    );
  }
}

// ================= 天气空状态（含城市设置入口）=================
/// 天气无数据时显示：错误提示 + 城市输入框。桌面/移动通用，
/// 解决桌面端原本无处设置城市、天气页永远空白的致命问题。
class _WeatherEmpty extends StatefulWidget {
  final String? error;
  const _WeatherEmpty({this.error});
  @override
  State<_WeatherEmpty> createState() => _WeatherEmptyState();
}

class _WeatherEmptyState extends State<_WeatherEmpty> {
  final _ctrl = TextEditingController();
  final _focus = FocusNode();

  @override
  void initState() {
    super.initState();
    _focus.addListener(() {
      context.read<AppState>().setInteractionLock(_focus.hasFocus);
    });
  }

  @override
  void dispose() {
    context.read<AppState>().setInteractionLock(false);
    _focus.dispose();
    _ctrl.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final s = context.read<AppState>();
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            widget.error ?? '设置城市后查看天气',
            style: const TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.dim),
            textAlign: TextAlign.center,
          ),
          const SizedBox(height: 8),
          SizedBox(
            width: 240,
            height: 28,
            child: TextField(
              controller: _ctrl,
              focusNode: _focus,
              style: const TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.white),
              decoration: InputDecoration(
                hintText: '输入城市名，回车查询',
                hintStyle: const TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.dim),
                filled: true, fillColor: DaozaiColors.track,
                contentPadding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
                border: OutlineInputBorder(borderRadius: BorderRadius.circular(8), borderSide: BorderSide.none),
                suffixIcon: GestureDetector(
                  onTap: () => s.refreshWeather(newCity: _ctrl.text),
                  child: const Icon(Icons.search, size: 16, color: DaozaiColors.cyan),
                ),
                suffixIconConstraints: const BoxConstraints(minWidth: 32, minHeight: 28),
              ),
              onSubmitted: (v) => s.refreshWeather(newCity: v),
            ),
          ),
        ],
      ),
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
  // 待确认删除的索引：点一次 × 进入确认态（变红），再点才真正删除，防误触。
  int? _confirmDelete;

  @override
  void initState() {
    super.initState();
    _focus.addListener(() {
      context.read<AppState>().setInteractionLock(_focus.hasFocus);
    });
  }

  @override
  void dispose() {
    context.read<AppState>().setInteractionLock(false);
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
                              if (_confirmDelete == i) {
                                s.removeTodo(i);
                                setState(() => _confirmDelete = null);
                              } else {
                                setState(() => _confirmDelete = i);
                              }
                            },
                            child: Icon(
                              _confirmDelete == i ? Icons.delete_forever : Icons.close,
                              size: 13,
                              color: _confirmDelete == i ? DaozaiColors.red : DaozaiColors.dim,
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

// ================= 专注 =================
class FocusPage extends StatelessWidget {
  const FocusPage({super.key});

  @override
  Widget build(BuildContext context) {
    final s = context.watch<AppState>();
    final total = (s.focusIsRest ? 5 : s.focusMinutes) * 60.0;
    final frac = total <= 0 ? 0.0 : (s.focusRemaining / total).clamp(0.0, 1.0);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        PageTitle('番茄', '专注', s.focusIsRest ? DaozaiColors.softGreen : DaozaiColors.redTop),
        const SizedBox(height: 4),
        Expanded(
          child: Row(
            children: [
              SizedBox(
                width: 92, height: 92,
                child: Stack(
                  alignment: Alignment.center,
                  children: [
                    SizedBox(
                      width: 88, height: 88,
                      child: CircularProgressIndicator(
                        value: frac, strokeWidth: 6,
                        color: s.focusIsRest ? DaozaiColors.softGreen : DaozaiColors.redTop,
                        backgroundColor: DaozaiColors.track,
                      ),
                    ),
                    Text(s.focusLabel, style: const TextStyle(fontSize: 20, fontWeight: FontWeight.w700, color: DaozaiColors.white)),
                  ],
                ),
              ),
              const SizedBox(width: 16),
              Expanded(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(s.focusIsRest ? '休息一下' : '专注时间', style: const TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.gray)),
                    if (s.focusMessage != null) ...[
                      const SizedBox(height: 3),
                      Text(s.focusMessage!, style: const TextStyle(fontSize: DaozaiSize.small, color: DaozaiColors.yellow)),
                    ],
                    const SizedBox(height: 8),
                    Row(
                      children: [
                        _pillBtn(s.focusRunning ? '暂停' : '开始', s.focusRunning ? DaozaiColors.btn : DaozaiColors.redTop, s.toggleFocus),
                        const SizedBox(width: 8),
                        _pillBtn('重置', DaozaiColors.btn, s.resetFocus),
                      ],
                    ),
                    const SizedBox(height: 8),
                    Row(
                      children: [15, 25, 45].map((m) => Padding(
                        padding: const EdgeInsets.only(right: 6),
                        child: GestureDetector(
                          onTap: () => s.setFocusMinutes(m),
                          child: Text('$m分', style: TextStyle(fontSize: DaozaiSize.small,
                              color: s.focusMinutes == m ? DaozaiColors.white : DaozaiColors.dim)),
                        ),
                      )).toList(),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

// ================= 喝水 =================
class WaterPage extends StatelessWidget {
  const WaterPage({super.key});

  @override
  Widget build(BuildContext context) {
    final s = context.watch<AppState>();
    final w = s.water;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const PageTitle('健康', '提醒', DaozaiColors.blue),
        const SizedBox(height: 4),
        Expanded(
          child: Row(
            children: [
              SizedBox(
                width: 92, height: 92,
                child: Stack(
                  alignment: Alignment.center,
                  children: [
                    SizedBox(
                      width: 88, height: 88,
                      child: CircularProgressIndicator(
                        value: w.percent / 100, strokeWidth: 6,
                        color: DaozaiColors.blue, backgroundColor: DaozaiColors.waterTrack,
                      ),
                    ),
                    Column(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Text('${w.drunkMl}', style: const TextStyle(fontSize: 18, fontWeight: FontWeight.w700, color: DaozaiColors.white)),
                        Text('/ ${w.goalMl} ml', style: const TextStyle(fontSize: DaozaiSize.tiny, color: DaozaiColors.gray)),
                      ],
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 16),
              Expanded(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text('今日饮水 ${w.percent.round()}%', style: const TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.gray)),
                    const SizedBox(height: 8),
                    Row(
                      children: [
                        _pillBtn('喝 ${w.servingMl}ml', DaozaiColors.blue, s.drink),
                        const SizedBox(width: 8),
                        _pillBtn('撤销', DaozaiColors.btn, s.undoDrink),
                      ],
                    ),
                    const SizedBox(height: 8),
                    // 目标与单次量可调（点 ± 调整），满足用户个性化需求。
                    Row(
                      children: [
                        _stepper('目标', w.goalMl, () => s.setWaterGoal(w.goalMl - 100), () => s.setWaterGoal(w.goalMl + 100)),
                        const SizedBox(width: 12),
                        _stepper('单次', w.servingMl, () => s.setWaterServing(w.servingMl - 50), () => s.setWaterServing(w.servingMl + 50)),
                      ],
                    ),
                  ],
                ),
              ),
            ],
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

Widget _pillBtn(String label, Color color, VoidCallback onTap) {
  return GestureDetector(
    onTap: onTap,
    child: Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 5),
      decoration: BoxDecoration(color: color, borderRadius: BorderRadius.circular(9)),
      child: Text(label, style: const TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.white, fontWeight: FontWeight.w600)),
    ),
  );
}

/// 带 ± 的数值调节器（喝水目标/单次量）。
Widget _stepper(String label, int value, VoidCallback onMinus, VoidCallback onPlus) {
  return Row(
    mainAxisSize: MainAxisSize.min,
    children: [
      Text('$label ', style: const TextStyle(fontSize: DaozaiSize.tiny, color: DaozaiColors.dim)),
      GestureDetector(onTap: onMinus, child: const Icon(Icons.remove_circle_outline, size: 14, color: DaozaiColors.gray)),
      Padding(
        padding: const EdgeInsets.symmetric(horizontal: 3),
        child: Text('$value', style: const TextStyle(fontSize: DaozaiSize.small, color: DaozaiColors.white)),
      ),
      GestureDetector(onTap: onPlus, child: const Icon(Icons.add_circle_outline, size: 14, color: DaozaiColors.gray)),
    ],
  );
}

/// 按当前页返回内容。
Widget pageBody(IslandPage page) {
  switch (page) {
    case IslandPage.weather: return const WeatherPage();
    case IslandPage.todo: return const TodoPage();
    case IslandPage.focus: return const FocusPage();
    case IslandPage.water: return const WaterPage();
  }
}