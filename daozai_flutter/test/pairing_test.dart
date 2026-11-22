//! 配对状态机回归测试 —— token 重放/过期/错码/超时/正常流转（验收 6）。
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:dynamic_island/core/pairing.dart';

void main() {
  final t0 = DateTime(2026, 1, 1, 12, 0, 0);

  test('SAS：同秘密同码，6 位补零；不同秘密大概率不同码', () {
    final a = Uint8List.fromList(List.generate(32, (i) => i));
    final b = Uint8List.fromList(List.generate(32, (i) => i + 1));
    expect(sasFromSharedSecret(a), sasFromSharedSecret(a));
    expect(sasFromSharedSecret(a).length, 6);
    expect(sasFromSharedSecret(a), isNot(sasFromSharedSecret(b)));
    expect(sasFromSharedSecret(Uint8List(32)), '000000');
  });

  test('token 校验：正确通过且一次性；重放拒绝', () {
    final s = PairingSession(token: 'tk', createdAt: t0);
    s.state = PairState.advertising;
    expect(s.consumeToken('tk', t0), isTrue);
    expect(s.state, PairState.connecting);
    expect(s.consumeToken('tk', t0), isFalse);
    expect(s.state, PairState.failed);
  });

  test('token 错误 → failed 且无残留', () {
    final s = PairingSession(token: 'tk', createdAt: t0);
    expect(s.consumeToken('wrong', t0), isFalse);
    expect(s.state, PairState.failed);
    expect(s.failReason, contains('错误'));
    expect(s.remoteName, isNull);
    expect(s.sas, isNull);
  });

  test('过期 token 拒绝；tick 超时把进行中会话打回 failed', () {
    final s = PairingSession(token: 'tk', createdAt: t0);
    expect(s.consumeToken('tk', t0.add(const Duration(seconds: 121))), isFalse);
    final s2 = PairingSession(token: 'tk2', createdAt: t0);
    s2.startJoin();
    s2.onKeyexDone('123456', 'B', 'fpB');
    s2.tick(t0.add(const Duration(seconds: 121)));
    expect(s2.state, PairState.failed);
    expect(s2.failReason, contains('超时'));
  });

  test('正常流转：join→keyex→SAS 确认→双方确认 fullyConfirmed', () {
    final s = PairingSession(token: 'tk', createdAt: t0);
    s.startJoin();
    s.onKeyexDone('482913', 'B', 'fpB');
    expect(s.state, PairState.sasShown);
    expect(s.confirm('482913'), isTrue);
    expect(s.fullyConfirmed, isFalse); // 还差对端确认
    s.onRemoteConfirm(true);
    expect(s.fullyConfirmed, isTrue);
  });

  test('SAS 不一致 → 拒绝并中止（防中间人）', () {
    final s = PairingSession(token: 'tk', createdAt: t0);
    s.startJoin();
    s.onKeyexDone('482913', 'B', 'fpB');
    expect(s.confirm('000000'), isFalse);
    expect(s.state, PairState.failed);
    expect(s.failReason, contains('不一致'));
  });

  test('对端取消 → failed；confirmed 后 cancel 不生效', () {
    final s = PairingSession(token: 'tk', createdAt: t0);
    s.startJoin();
    s.onKeyexDone('111111', 'B', 'fpB');
    s.onRemoteConfirm(false);
    expect(s.state, PairState.failed);
    final s2 = PairingSession(token: 'tk2', createdAt: t0);
    s2.startJoin();
    s2.onKeyexDone('222222', 'C', 'fpC');
    s2.confirm('222222');
    s2.cancel();
    expect(s2.state, PairState.confirmed);
  });

  test('generatePairToken：随机不重复、URL 安全', () {
    final a = generatePairToken();
    final b = generatePairToken();
    expect(a, isNot(b));
    expect(a.contains('='), isFalse);
    expect(a.length, greaterThanOrEqualTo(16));
  });
}