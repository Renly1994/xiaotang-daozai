//! 配色与字号 —— 忠实复刻原项目 src/theme.rs 的逐像素取色。
import 'package:flutter/material.dart';

/// 岛仔配色（原版逐像素取色，少量为审美微调）。
class DaozaiColors {
  DaozaiColors._();

  static const Color red = Color.fromRGBO(242, 48, 70, 1);
  static const Color redTop = Color.fromRGBO(253, 104, 92, 1);
  static const Color blue = Color.fromRGBO(62, 172, 248, 1);
  static const Color blueDeep = Color.fromRGBO(13, 69, 128, 1);
  static const Color cyan = Color.fromRGBO(102, 219, 255, 1);
  static const Color softGreen = Color.fromRGBO(118, 196, 151, 1);
  static const Color green = Color.fromRGBO(48, 209, 88, 1);
  static const Color sitGreen = Color.fromRGBO(70, 200, 125, 1);
  static const Color charge = Color.fromRGBO(33, 235, 84, 1);
  static const Color white = Color.fromRGBO(255, 255, 255, 1);
  static const Color gray = Color.fromRGBO(178, 178, 186, 1);
  static const Color dim = Color.fromRGBO(142, 142, 153, 1);
  static const Color btn = Color.fromRGBO(40, 40, 43, 1);
  static const Color btnHover = Color.fromRGBO(56, 56, 60, 1);
  static const Color card = Color.fromRGBO(22, 22, 24, 1);
  static const Color track = Color.fromRGBO(36, 36, 39, 1);
  static const Color arrowBg = Color.fromRGBO(48, 48, 51, 1);
  static const Color yellow = Color.fromRGBO(255, 196, 61, 1);

  /// 岛体底色（经典黑）。
  static const Color islandBody = Color.fromRGBO(18, 18, 20, 1);

  static Color alphaBlue(double a) =>
      blue.withOpacity(a.clamp(0.0, 1.0));
}

/// 字号表（逻辑点）—— 对应原项目 theme::size。
class DaozaiSize {
  DaozaiSize._();
  static const double title = 14.5; // 页标题（中文）
  static const double date = 11.0; // 顶栏日期
  static const double clock = 12.5; // 顶栏时钟
  static const double pillClock = 15.5; // 药丸时钟
  static const double hero = 29.0; // 大数字
  static const double body = 11.5; // 正文、按钮
  static const double small = 11.0; // 说明
  static const double tiny = 10.0; // 脚注
}

/// 几何常量 —— 对应原项目 app.rs 顶部。
class DaozaiGeom {
  DaozaiGeom._();
  static const double panelW = 419.0; // 展开面板宽
  static const double panelH = 140.0; // 展开面板高
  static const double barW = 163.0; // 收起药丸宽
  static const double barH = 30.0; // 收起药丸高
  static const double flare = 17.6; // 顶部外翻肩宽
  static const double flareH = 16.0; // 外翻收敛高度
  static const double rad = 33.0; // 底部圆角
  static const double pillGap = 6.0; // 胶囊离屏幕顶边间隙
}

/// 全局深色主题。
ThemeData buildDaozaiTheme() {
  final base = ThemeData.dark(useMaterial3: true);
  return base.copyWith(
    scaffoldBackgroundColor: Colors.transparent,
    colorScheme: base.colorScheme.copyWith(
      primary: DaozaiColors.blue,
      secondary: DaozaiColors.cyan,
      surface: DaozaiColors.card,
    ),
    textTheme: base.textTheme.apply(
      bodyColor: DaozaiColors.white,
      displayColor: DaozaiColors.white,
      fontFamily: 'Segoe UI',
    ),
    splashFactory: NoSplash.splashFactory,
    highlightColor: Colors.transparent,
  );
}