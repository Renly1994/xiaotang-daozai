//! 岛体主组件 —— 对应原项目 app.rs 的 frame/morph（骨架精简版）。
//! 收起药丸 ↔ 展开面板用复刻的二阶阻尼弹簧驱动，悬停意图缓冲防误触。
import 'dart:io' show Platform;
import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';
import 'package:provider/provider.dart';
import 'package:window_manager/window_manager.dart';
import '../core/theme.dart';
import '../core/spring.dart';
import '../state/app_state.dart';
import '../core/quit.dart';
import '../core/window_pos.dart';
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
  Ticker? _ticker;
  double _lastT = 0;
  double _hoverSince = -1;
  double _leaveSince = -1;
  bool _open = false;
  double _now = 0;
  // 鼠标是否在岛体感应区内（由全局鼠标位置计算，供 Ticker 确定性判断）。
  bool _inside = false;
  // 当前是否处于点击穿透状态（岛体外穿透到下层窗口）。
  bool _ignoring = false;
  // forward 鼠标转发仅 Windows/macOS 支持；Linux 不支持，启用穿透会导致岛体点不到，
  // 故 Linux 保持整窗交互（不穿透）。
  static final bool _supportsForward = Platform.isWindows || Platform.isMacOS;

  // 展开目标尺寸（含肩部余量）。
  static const double _openW = DaozaiGeom.panelW + DaozaiGeom.flare * 2 + 24;
  static const double _openH = DaozaiGeom.panelH + 24;

  @override
  void initState() {
    super.initState();
    _w = Spring(DaozaiGeom.barW, DaozaiMotion.closeK, DaozaiMotion.closeZ);
    _h = Spring(DaozaiGeom.barH, DaozaiMotion.closeK, DaozaiMotion.closeZ);
    if (!widget.floating) {
      // 移动 App 模式：常驻展开，弹簧直接 snap 到位，无需 Ticker 每帧驱动（省电）。
      _open = true;
      _w.snap(_openW);
      _h.snap(_openH);
    } else {
      _ticker = createTicker(_onTick)..start();
      // 桌面悬浮：启动即穿透（鼠标不在岛体上时不挡桌面），移到岛体上再恢复交互。
      // 仅 Windows/macOS 支持 forward 转发；Linux 不穿透。
      if (_supportsForward) {
        _ignoring = true;
        windowManager.setIgnoreMouseEvents(true, forward: true);
      }
    }
  }

  void _onTick(Duration elapsed) {
    final t = elapsed.inMicroseconds / 1e6;
    final dt = _lastT == 0 ? 0.0 : (t - _lastT).clamp(0.0, 0.1);
    _lastT = t;
    _now = t;
    if (widget.floating) {
      final s = context.read<AppState>();
      // 到点提醒等外部展开意图：AppState.expanded 为真时岛体必须真的展开。
      if (s.expanded && !_open) _open = true;
      _updateHover(t);
      _morph(dt);
    }
    _w.step(dt);
    _h.step(dt);
    if (mounted) setState(() {});
    // 空闲停摆：收起、无悬停、无锁定且弹簧已静止 → 停 Ticker 省电。
    // 指针移动 / 展开 / 锁定变化时会重新启动（见 _ensureTicker）。
    if (widget.floating &&
        !_open &&
        !_inside &&
        _hoverSince < 0 &&
        _leaveSince < 0 &&
        !context.read<AppState>().interactionLock &&
        _w.settled(DaozaiGeom.barW) &&
        _h.settled(DaozaiGeom.barH)) {
      _ticker?.stop();
    }
  }

  void _ensureTicker() {
    if (!widget.floating) return;
    _ticker ??= createTicker(_onTick);
    if (!_ticker!.isActive) _ticker!.start();
  }

  /// 悬停意图缓冲（确定性状态机，对齐原项目 morph 的 hover_since/leave_since）：
  /// 光标停留 hoverIntent 才展开；移开超过 leaveDelay 才收起。
  /// 在每帧 Ticker 里判断，避免延迟回调与帧时序竞态导致误展开/误收起。
  void _updateHover(double t) {
    final locked = context.read<AppState>().interactionLock;
    if (_inside || locked) {
      _leaveSince = -1;
      if (!_open) {
        if (_hoverSince < 0) _hoverSince = t;
        if (t - _hoverSince >= DaozaiMotion.hoverIntent || locked) {
          _open = true;
          context.read<AppState>().setExpanded(true);
        }
      }
    } else {
      _hoverSince = -1;
      if (_open) {
        if (_leaveSince < 0) _leaveSince = t;
        if (t - _leaveSince >= DaozaiMotion.leaveDelay) {
          _open = false;
          context.read<AppState>().setExpanded(false);
        }
      }
    }
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

  void _onExit() {
    if (!widget.floating) return;
    // 仅作安全网：指针真正离开岛体外框时清标志。
    // 进入（onEnter）不写 _inside——命中与否只由指针坐标判定，避免双源竞态。
    _inside = false;
  }

  /// 岛体命中矩形（窗口局部坐标）：水平居中、顶部对齐，尺寸随展开进度变化。
  Rect _islandRect() {
    final w = _w.pos;
    final h = _h.pos;
    final left = (_openW - w) / 2;
    return Rect.fromLTWH(left, 0, w, h);
  }

  /// 圆角近似命中：底部大圆角用 RRect 判定，不再让透明四角挡住桌面点击。
  /// 顶部肩部仍为矩形近似（误差小），完整轮廓匹配留待原生通道。
  bool _hitIsland(Offset p) {
    final r = _islandRect();
    final bottomRad = math.min(DaozaiGeom.rad, r.height * 0.5);
    final rr = RRect.fromRectAndCorners(
      r,
      bottomLeft: Radius.circular(bottomRad),
      bottomRight: Radius.circular(bottomRad),
      topLeft: const Radius.circular(6),
      topRight: const Radius.circular(6),
    );
    return rr.contains(p);
  }

  /// 全局鼠标移动驱动点击穿透：
  /// 鼠标在岛体内 → 恢复交互；在岛体外 → 穿透到下层（forward 继续转发移动事件）。
  /// 穿透状态下 MouseRegion 不触发，故悬停展开也由此处的 _inside 驱动。
  void _onPointerMove(PointerEvent event) {
    if (!widget.floating) return;
    final hit = _hitIsland(event.localPosition);
    _inside = hit;
    // 只有需要动画/判定时才唤醒 Ticker，避免空闲时每次划过都启动一帧。
    if (hit || _open || context.read<AppState>().interactionLock) _ensureTicker();
    // Linux 不支持 forward，不做穿透切换（保持整窗交互）。
    if (!_supportsForward) return;
    final wantIgnore = !hit;
    if (wantIgnore != _ignoring) {
      _ignoring = wantIgnore;
      windowManager.setIgnoreMouseEvents(wantIgnore, forward: true);
    }
  }

  @override
  void dispose() {
    _ticker?.dispose();
    // 退出时恢复鼠标事件，避免残留整窗穿透状态。
    if (widget.floating && _supportsForward && _ignoring) {
      windowManager.setIgnoreMouseEvents(false);
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final s = context.watch<AppState>();
    // 空闲停摆后，锁定/展开/悬停任一成立时唤醒 Ticker（post-frame 避免 build 中副作用）。
    if (widget.floating &&
        (_open || _inside || s.interactionLock || s.expanded) &&
        !(_ticker?.isActive ?? false)) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _ensureTicker();
      });
    }
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
              ? Stack(
                  children: [
                    Padding(
                      padding: const EdgeInsets.fromLTRB(16, 12, 16, 10),
                      child: Column(
                        children: [
                          Expanded(child: pageBody(s.page)),
                          const SizedBox(height: 4),
                          _PageDots(current: s.page, onTap: s.setPage),
                        ],
                      ),
                    ),
                    // 桌面悬浮模式：右上角退出按钮（悬浮窗无标题栏，需显式退出入口）。
                    if (widget.floating)
                      Positioned(
                        top: 2,
                        right: 2,
                        child: GestureDetector(
                          onTap: quitApp,
                          child: Container(
                            width: 22,
                            height: 22,
                            decoration: BoxDecoration(
                              color: DaozaiColors.btn,
                              borderRadius: BorderRadius.circular(6),
                            ),
                            child: const Icon(Icons.close, size: 13, color: DaozaiColors.gray),
                          ),
                        ),
                      ),
                  ],
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
              // 收起药丸仅显示时钟（天气功能已移除）。
              Text(s.clock, style: const TextStyle(fontSize: DaozaiSize.pillClock, fontWeight: FontWeight.w600, color: DaozaiColors.white)),
            ],
          ),
        ),
      ),
    );

    return Listener(
      onPointerHover: _onPointerMove,
      onPointerMove: _onPointerMove,
      child: MouseRegion(
        onExit: (_) => _onExit(),
        child: GestureDetector(
        onTap: () {
          if (widget.floating && !_open) {
            _ensureTicker();
            setState(() => _open = true);
            context.read<AppState>().setExpanded(true);
          }
        },
        // 收起态按住药丸拖动 = 移动整个窗口（原生跟手拖动，位置由系统管理）。
        // 展开态不触发拖动，避免与面板内操作冲突。
        onPanStart: (_) {
          if (widget.floating && !_open) {
            windowManager.startDragging();
            // 原生拖动结束后保存位置（拖动期间位置持续变化，延迟到松手后保存）。
            Future.delayed(const Duration(milliseconds: 600), saveWindowPosition);
          }
        },
        // 拖动结束（松手）再保存一次，确保最终位置落盘。
        onPanEnd: (_) {
          if (widget.floating) saveWindowPosition();
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