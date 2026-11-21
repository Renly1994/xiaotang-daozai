//! 岛体主组件 —— 对应原项目 app.rs 的 frame/morph（骨架精简版）。
//! 收起药丸 ↔ 展开面板用复刻的二阶阻尼弹簧驱动，悬停意图缓冲防误触。
import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';
import 'package:provider/provider.dart';
import '../core/theme.dart';
import '../core/spring.dart';
import '../core/weather_code.dart';
import '../state/app_state.dart';
import 'island_shape.dart';
import 'pages.dart';

class IslandWidget extends StatefulWidget {
  /// 桌面悬浮模式：透明背景、悬停展开；移动 App 模式：常驻展开卡片。
  final bool floating;
  const IslandWidget({super.key, this.floating = true});

  @override
  State<IslandWidget> createState() => _IslandWidgetState();
}

class _IslandWidgetState extends State<IslandWidget> with SingleTickerProviderStateMixin {
  late final Spring _w;
  late final Spring _h;
  late final Ticker _ticker;
  double _lastT = 0;
  double _hoverSince = -1;
  double _leaveSince = -1;
  bool _open = false;
  double _now = 0;

  // 展开目标尺寸（含肩部余量）。
  static const double _openW = DaozaiGeom.panelW + DaozaiGeom.flare * 2 + 24;
  static const double _openH = DaozaiGeom.panelH + 24;

  @override
  void initState() {
    super.initState();
    _w = Spring(DaozaiGeom.barW, DaozaiMotion.closeK, DaozaiMotion.closeZ);
    _h = Spring(DaozaiGeom.barH, DaozaiMotion.closeK, DaozaiMotion.closeZ);
    if (!widget.floating) {
      _open = true;
      _w.snap(_openW);
      _h.snap(_openH);
    }
    _ticker = createTicker(_onTick)..start();
  }

  void _onTick(Duration elapsed) {
    final t = elapsed.inMicroseconds / 1e6;
    final dt = _lastT == 0 ? 0.0 : (t - _lastT).clamp(0.0, 0.1);
    _lastT = t;
    _now = t;
    if (widget.floating) {
      _morph(dt);
    }
    _w.step(dt);
    _h.step(dt);
    if (mounted) setState(() {});
  }

  void _morph(double dt) {
    final want = _open;
    if (want) {
      _leaveSince = -1;
      _w.tune(DaozaiMotion.openWk, DaozaiMotion.openWz);
      _h.tune(DaozaiMotion.openHk, DaozaiMotion.openHz);
      _w.target = _openW;
      _h.target = _openH;
    } else {
      _hoverSince = -1;
      _w.tune(DaozaiMotion.closeK, DaozaiMotion.closeZ);
      _h.tune(DaozaiMotion.closeK, DaozaiMotion.closeZ);
      _w.target = DaozaiGeom.barW;
      _h.target = DaozaiGeom.barH;
    }
  }

  void _onEnter() {
    if (!widget.floating) return;
    _leaveSince = -1;
    _hoverSince = _now;
    // 意图缓冲：停留 0.2s 才展开（骨架阶段用延迟近似）。
    Future.delayed(const Duration(milliseconds: 200), () {
      if (mounted && _hoverSince >= 0 && _now - _hoverSince >= DaozaiMotion.hoverIntent - 0.02) {
        setState(() => _open = true);
        context.read<AppState>().setExpanded(true);
      }
    });
  }

  void _onExit() {
    if (!widget.floating) return;
    _hoverSince = -1;
    _leaveSince = _now;
    Future.delayed(const Duration(milliseconds: 120), () {
      if (mounted && _leaveSince >= 0 && _now - _leaveSince >= DaozaiMotion.leaveDelay - 0.02) {
        setState(() => _open = false);
        context.read<AppState>().setExpanded(false);
      }
    });
  }

  @override
  void dispose() {
    _ticker.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final s = context.watch<AppState>();
    final progress = ((_w.pos - DaozaiGeom.barW) / (_openW - DaozaiGeom.barW)).clamp(0.0, 1.0);
    final reveal = smoothstep(0.5, 0.97, progress);
    final bodyW = _w.pos;
    final bodyH = _h.pos;

    final content = AnimatedOpacity(
      opacity: reveal,
      duration: const Duration(milliseconds: 80),
      child: IgnorePointer(
        ignoring: reveal < 0.6,
        child: SizedBox(
          width: DaozaiGeom.panelW,
          height: DaozaiGeom.panelH,
          child: _open || !widget.floating
              ? Padding(
                  padding: const EdgeInsets.fromLTRB(16, 12, 16, 10),
                  child: Column(
                    children: [
                      Expanded(child: pageBody(s.page)),
                      const SizedBox(height: 4),
                      _PageDots(current: s.page, onTap: s.setPage),
                    ],
                  ),
                )
              : const SizedBox.shrink(),
        ),
      ),
    );

    final pill = AnimatedOpacity(
      opacity: (1 - smoothstep(0.0, 0.3, progress)),
      duration: const Duration(milliseconds: 80),
      child: IgnorePointer(
        ignoring: progress > 0.3,
        child: SizedBox(
          width: DaozaiGeom.barW,
          height: DaozaiGeom.barH,
          child: Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              const Icon(Icons.battery_full, size: 14, color: DaozaiColors.gray),
              const SizedBox(width: 8),
              Text(s.clock, style: const TextStyle(fontSize: DaozaiSize.pillClock, fontWeight: FontWeight.w600, color: DaozaiColors.white)),
              if (s.weather != null) ...[
                const SizedBox(width: 8),
                Text('${weatherEmoji(s.weather!.kind)} ${s.weather!.temp.round()}°',
                    style: const TextStyle(fontSize: DaozaiSize.small, color: DaozaiColors.gray)),
              ],
            ],
          ),
        ),
      ),
    );

    return MouseRegion(
      onEnter: (_) => _onEnter(),
      onExit: (_) => _onExit(),
      child: GestureDetector(
        onTap: () {
          if (widget.floating && !_open) {
            setState(() => _open = true);
            context.read<AppState>().setExpanded(true);
          }
        },
        child: SizedBox(
          width: _openW,
          height: _openH,
          child: Stack(
            alignment: Alignment.topCenter,
            children: [
              CustomPaint(
                size: Size(_openW, _openH),
                painter: IslandBodyPainter(
                  w: bodyW, h: bodyH, progress: progress,
                  pill: false,
                ),
              ),
              // 内容居中于岛体（肩部在两侧，内容从顶部 fh 处开始）。
              Positioned(
                top: DaozaiGeom.flareH * progress + 2,
                child: SizedBox(
                  width: bodyW,
                  height: bodyH,
                  child: Stack(
                    alignment: Alignment.center,
                    children: [pill, content],
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _PageDots extends StatelessWidget {
  final IslandPage current;
  final ValueChanged<IslandPage> onTap;
  const _PageDots({required this.current, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: IslandPage.values.map((p) {
        final on = p == current;
        return GestureDetector(
          onTap: () => onTap(p),
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 200),
            margin: const EdgeInsets.symmetric(horizontal: 2),
            width: on ? 16 : 4,
            height: 4,
            decoration: BoxDecoration(
              color: on ? DaozaiColors.white : DaozaiColors.dim,
              borderRadius: BorderRadius.circular(2),
            ),
          ),
        );
      }).toList(),
    );
  }
}