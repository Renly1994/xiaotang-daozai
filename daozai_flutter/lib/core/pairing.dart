//! 配对状态机 —— 纯逻辑：token 生成/校验、TTL、SAS 计算、状态流转（可单测）。
//! 加密原语（ECDH/签名）由调用方注入结果，本文件只做协议决策，不碰密钥材料。
import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';
import '../services/ports.dart';

enum PairState { idle, advertising, connecting, keyex, sasShown, confirmed, failed }

/// SAS：共享秘密 SHA-256（调用方算好传入）截断 mod 10^6 → 6 位数字。
/// 两端用同一共享秘密必然得到同码；中间人秘密不同 → 码不同 → 用户比对发现。
String sasFromSharedSecret(Uint8List sha256OfSecret) {
  final bd = ByteData.sublistView(sha256OfSecret, 0, 8);
  final v = bd.getUint64(0, Endian.big) % 1000000;
  return v.toString().padLeft(6, '0');
}

/// 一次性配对 token：随机 16 字节 hex。
String generatePairToken([Random? rng]) {
  final r = rng ?? Random.secure();
  final bytes = List<int>.generate(16, (_) => r.nextInt(256));
  return base64Url.encode(bytes).replaceAll('=', '');
}

class PairingSession implements PairingSessionView {
  @override
  final String token;
  final DateTime createdAt;
  final Duration ttl;
  PairState state = PairState.idle;
  @override
  String? remoteName;
  @override
  String? remoteFingerprint;
  @override
  String? sas;
  @override
  String? failReason;
  bool _tokenUsed = false;

  @override
  bool get isFailed => state == PairState.failed;
  @override
  bool get isConfirmed => state == PairState.confirmed;

  PairingSession({
    required this.token,
    required this.createdAt,
    this.ttl = const Duration(seconds: 120),
  });

  bool expired(DateTime now) => now.difference(createdAt) > ttl;

  /// 校验连入方 token：必须匹配、未用过、未过期。用一次即作废（防重放）。
  bool consumeToken(String presented, DateTime now) {
    if (expired(now)) {
      _fail('配对已超时，请重新发起');
      return false;
    }
    if (_tokenUsed) {
      _fail('配对码已被使用');
      return false;
    }
    if (presented != token) {
      _fail('配对码错误');
      return false;
    }
    _tokenUsed = true;
    state = PairState.connecting;
    return true;
  }

  /// 主动加入方：直接进 connecting（token 由二维码/手动输入带来，对端校验）。
  void startJoin() {
    if (state != PairState.idle) return;
    state = PairState.connecting;
  }

  void onKeyexDone(String sasCode, String name, String fingerprint) {
    if (state != PairState.connecting) return;
    sas = sasCode;
    remoteName = name;
    remoteFingerprint = fingerprint;
    state = PairState.sasShown;
  }

  /// 用户比对两端 SAS 一致后点确认。码不一致 → 拒绝（可能中间人）。
  bool confirm(String userEnteredSas) {
    if (state != PairState.sasShown) return false;
    if (userEnteredSas != sas) {
      _fail('验证码不一致，连接已中止（可能不安全）');
      return false;
    }
    state = PairState.confirmed;
    return true;
  }

  /// 对端确认到达（双方都确认才算配对成功）。
  /// 顺序无关：对端确认可能先于本端用户点确认到达，先记下，confirm 时合流。
  bool _remoteConfirmed = false;
  void onRemoteConfirm(bool ok) {
    if (!ok) {
      _fail('对方取消了配对');
      return;
    }
    _remoteConfirmed = true;
  }

  @override
  bool get fullyConfirmed => state == PairState.confirmed && _remoteConfirmed;

  void tick(DateTime now) {
    if (state == PairState.confirmed || state == PairState.failed || state == PairState.idle) return;
    if (expired(now)) _fail('配对已超时，请重新发起');
  }

  void cancel() {
    if (state == PairState.confirmed) return;
    _fail('配对已取消');
  }

  void _fail(String reason) {
    state = PairState.failed;
    failReason = reason;
    // 无半配对残留：清掉对端信息与 SAS。
    remoteName = null;
    remoteFingerprint = null;
    sas = null;
  }
}