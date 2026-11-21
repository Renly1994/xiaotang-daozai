//! 岛体轮廓 —— 复刻原项目 src/shape.rs：顶部外翻「肩」+ 竖直侧边 + 底部大圆角。
//! 用 CustomPainter 按行扫描填充，肩部用二次收敛曲线，边缘抗锯齿交给 Flutter。
import 'dart:math' as math;
import 'package:flutter/material.dart';
import '../core/theme.dart';

/// 生成刘海面板的闭合轮廓路径（局部坐标，原点在面板左上角）。
/// [w]/[h] 当前宽高，[flare]/[flareH] 肩部参数，[radius] 底部圆角。
Path islandOutline(double w, double h, double flare, double flareH, double radius) {
  final r = math.min(radius, math.min(w, h) * 0.5);
  final fh = math.min(flareH, h * 0.5);
  final path = Path();
  // 左上肩起点（外翻最宽处）
  path.moveTo(-flare, 0);
  // 左肩：从外翻端收敛到竖直侧边（二次曲线，对应原版 powf(2.4) 近似）
  path.quadraticBezierTo(-flare * 0.25, fh * 0.7, 0, fh);
  // 左侧竖直边到底部圆角起点
  path.lineTo(0, h - r);
  // 左下圆角
  path.quadraticBezierTo(0, h, r, h);
  // 底边
  path.lineTo(w - r, h);
  // 右下圆角
  path.quadraticBezierTo(w, h, w, h - r);
  // 右侧竖直边到肩
  path.lineTo(w, fh);
  // 右肩
  path.quadraticBezierTo(w + flare * 0.25, fh * 0.7, w + flare, 0);
  // 顶边闭合
  path.close();
  return path;
}

/// 胶囊（收起态）轮廓：纯圆角矩形。
Path pillOutline(double w, double h) {
  final r = h * 0.5;
  return Path()
    ..addRRect(RRect.fromRectAndRadius(Rect.fromLTWH(0, 0, w, h), Radius.circular(r)));
}

class IslandBodyPainter extends CustomPainter {
  final double w;
  final double h;
  final double progress; // 0=收起药丸，1=展开面板
  final Color fill;
  final bool pill;

  IslandBodyPainter({
    required this.w,
    required this.h,
    required this.progress,
    this.fill = DaozaiColors.islandBody,
    this.pill = false,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final path = pill
        ? pillOutline(w, h)
        : islandOutline(
            w, h,
            DaozaiGeom.flare * progress,
            DaozaiGeom.flareH * progress,
            DaozaiGeom.rad * progress.clamp(0.3, 1.0),
          );
    // 柔和投影（多层叠加近似高斯，对应原版 pill_shadow / shape::shadow）。
    final shadowPaint = Paint()..color = Colors.black.withOpacity(0.10);
    canvas.save();
    canvas.translate(0, 3);
    canvas.drawPath(path, shadowPaint);
    canvas.restore();

    final paint = Paint()
      ..color = fill
      ..style = PaintingStyle.fill
      ..isAntiAlias = true;
    canvas.drawPath(path, paint);

    // 顶部高光描边，增加层次。
    final rim = Paint()
      ..color = Colors.white.withOpacity(0.06)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1
      ..isAntiAlias = true;
    canvas.drawPath(path, rim);
  }

  @override
  bool shouldRepaint(covariant IslandBodyPainter old) =>
      old.w != w || old.h != h || old.progress != progress || old.fill != fill || old.pill != pill;
}