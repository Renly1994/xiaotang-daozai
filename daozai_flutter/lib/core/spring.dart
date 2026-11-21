//! 动效基础件 —— 忠实复刻原项目 src/anim.rs。
//!
//! 所有形变都用「带速度的弹簧」驱动：目标随时可改，速度保留，
//! 所以展开途中移开、收起途中回来都不会跳变，这是「丝滑」的关键。
import 'dart:math' as math;

/// 二阶阻尼弹簧 `x'' = -k(x - target) - c·x'`，半隐式欧拉 + 子步长积分。
class Spring {
  double pos;
  double vel;
  double target;
  double _k;
  double _c;

  /// [stiffness] = k，[zeta] = 阻尼比（<1 有过冲，=1 临界阻尼）。
  Spring(double value, double stiffness, double zeta)
      : pos = value,
        vel = 0,
        target = value,
        _k = 0,
        _c = 0 {
    tune(stiffness, zeta);
  }

  void tune(double stiffness, double zeta) {
    _k = stiffness;
    _c = 2.0 * zeta * math.sqrt(stiffness);
  }

  void snap(double value) {
    pos = value;
    target = value;
    vel = 0;
  }

  void step(double dt) {
    // 掉帧时限幅，避免一帧跨太远；子步 ≤ 1/480 s，保证在 30–240 Hz 下轨迹一致。
    final clamped = dt.clamp(0.0, 0.1);
    final n = math.max(1, (clamped * 480.0).ceil());
    final h = clamped / n;
    for (var i = 0; i < n; i++) {
      final a = -_k * (pos - target) - _c * vel;
      vel += a * h;
      pos += vel * h;
    }
  }

  /// 相对 [scale] 足够接近目标且几乎静止。
  bool settled(double scale) =>
      (pos - target).abs() < 0.002 * scale && vel.abs() < 0.02 * scale;
}

double lerp(double a, double b, double t) => a + (b - a) * t;

double clamp01(double t) => t.clamp(0.0, 1.0);

/// 把 x 从 [a,b] 线性映射到 [0,1] 并钳制；允许 a>b（反向区间）。
double remap(double x, double a, double b) {
  final d = b - a;
  if (d.abs() < 1e-6) return x >= b ? 1.0 : 0.0;
  return clamp01((x - a) / d);
}

double smoothstep(double a, double b, double x) {
  final t = remap(x, a, b);
  return t * t * (3.0 - 2.0 * t);
}

double easeOutCubic(double t) {
  final u = 1.0 - clamp01(t);
  return 1.0 - u * u * u;
}

/// 帧率无关的指数逼近：rate 越大越快（约 1/rate 秒到 63%）。
double approach(double cur, double target, double rate, double dt) =>
    cur + (target - cur) * (1.0 - math.exp(-rate * dt));

/// 动效参数 —— 对应原项目 app.rs。
class DaozaiMotion {
  DaozaiMotion._();
  // 展开：欠阻尼，末段约 5% 过冲后回落；高度略软于宽度，像「垂落」下来。
  static const double openWk = 330.0, openWz = 0.70;
  static const double openHk = 290.0, openHz = 0.74;
  // 收起：临界阻尼，约 0.18 s 干净收回，无回弹。
  static const double closeK = 620.0, closeZ = 1.0;
  // 悬停意图缓冲（秒）：光标停留这么久才真正展开，防误触。
  static const double hoverIntent = 0.20;
  // 移开后等待多久开始收起。
  static const double leaveDelay = 0.12;
}