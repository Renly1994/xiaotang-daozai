//! 核心纯逻辑回归测试 —— 验证从 Rust 复刻的弹簧 / 验证码打分。
//! 天气映射测试已随天气功能移除；验证码逻辑保留供后续邮箱功能复用。
import 'package:flutter_test/flutter_test.dart';
import 'package:daozai_flutter/core/spring.dart';
import 'package:daozai_flutter/core/otp.dart';

void main() {
  group('Spring（复刻 anim.rs）', () {
    test('欠阻尼弹簧过冲后回落', () {
      final s = Spring(0, 320, 0.7);
      s.target = 1;
      var peak = 0.0;
      for (var i = 0; i < 120; i++) {
        s.step(1 / 60);
        peak = peak > s.pos ? peak : s.pos;
      }
      expect(peak, greaterThan(1.02));
      expect(peak, lessThan(1.08));
      expect(s.settled(1), isTrue);
    });

    test('临界阻尼不过冲', () {
      final s = Spring(1, 560, 1.0);
      s.target = 0;
      for (var i = 0; i < 60; i++) {
        s.step(1 / 60);
        expect(s.pos, greaterThan(-1e-4));
      }
    });

    test('帧率无关', () {
      final a = Spring(0, 300, 0.75);
      final b = Spring(0, 300, 0.75);
      a.target = 1;
      b.target = 1;
      for (var i = 0; i < 30; i++) { a.step(1 / 30); }
      for (var i = 0; i < 144; i++) { b.step(1 / 144); }
      expect((a.pos - b.pos).abs(), lessThan(0.01));
    });
  });

  group('验证码打分（复刻 otp.rs）', () {
    test('标准验证码可识别', () {
      final code = extractOtp('您的验证码', '验证码是 123456，5分钟内有效。', []);
      expect(code, isNotNull);
      expect(code!.text, '123456');
      expect(code.score, greaterThanOrEqualTo(kOtpThreshold));
    });
    test('订单号不误判', () {
      final code = extractOtp('订单发货通知', '您的订单号 12345678 已发货，金额 ¥128.00。', []);
      expect(code, isNull);
    });
    test('年份不误判', () {
      final code = extractOtp('年度回顾', '回顾 2026 年的点滴。', []);
      expect(code, isNull);
    });
    test('HTML 转纯文本', () {
      expect(htmlToText('<p>验证码 <b>888999</b></p><script>x</script>'), contains('888999'));
    });
  });
}