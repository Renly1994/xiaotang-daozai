//! 密钥安全存储 —— flutter_secure_storage 实现 KeyVault。
//! macOS Keychain / Windows DPAPI / Android Keystore；测试注入内存 fake。
//! 私钥永不离开安全存储；本文件只在内存中短暂持有用于握手/加密。
import 'dart:convert';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'ports.dart';

class SecureKeys implements KeyVault {
  final FlutterSecureStorage _storage;
  static const _kIdentity = 'daozai.identity';
  static const _kTrusted = 'daozai.trusted';
  static const _kVaultKey = 'daozai.vaultkey';
  static const _kDeviceName = 'daozai.devicename';
  DeviceIdentity? _cached;

  SecureKeys({FlutterSecureStorage? storage})
      : _storage = storage ??
            const FlutterSecureStorage(
              // 11.x 默认即 AES-GCM + RSA-OAEP 密钥包裹（Keystore），无需旧版 encryptedSharedPreferences。
              aOptions: AndroidOptions(),
            );

  @override
  Future<DeviceIdentity> loadOrCreate() async {
    if (_cached != null) return _cached!;
    final raw = await _storage.read(key: _kIdentity);
    if (raw != null) {
      final j = jsonDecode(raw) as Map<String, dynamic>;
      _cached = DeviceIdentity(
        ed25519Private: base64Decode(j['edPriv'] as String),
        ed25519Public: base64Decode(j['edPub'] as String),
        x25519Private: base64Decode(j['xPriv'] as String),
        x25519Public: base64Decode(j['xPub'] as String),
        fingerprint: j['fp'] as String,
      );
      return _cached!;
    }
    final ed = await Ed25519().newKeyPair();
    final edPriv = await ed.extractPrivateKeyBytes();
    final edPub = (await ed.extractPublicKey()).bytes;
    final x = await X25519().newKeyPair();
    final xPriv = await x.extractPrivateKeyBytes();
    final xPub = (await x.extractPublicKey()).bytes;
    final hash = await Sha256().hash(edPub);
    final fp = _hex(hash.bytes.sublist(0, 8));
    _cached = DeviceIdentity(
      ed25519Private: Uint8List.fromList(edPriv),
      ed25519Public: Uint8List.fromList(edPub),
      x25519Private: Uint8List.fromList(xPriv),
      x25519Public: Uint8List.fromList(xPub),
      fingerprint: fp,
    );
    await _storage.write(
      key: _kIdentity,
      value: jsonEncode({
        'edPriv': base64Encode(edPriv),
        'edPub': base64Encode(edPub),
        'xPriv': base64Encode(xPriv),
        'xPub': base64Encode(xPub),
        'fp': fp,
      }),
    );
    return _cached!;
  }

  static String _hex(List<int> bytes) =>
      bytes.map((b) => b.toRadixString(16).padLeft(2, '0')).join().toUpperCase();

  /// 指纹展示格式：A1B2 C3D4 E5F6 0718。
  static String prettyFingerprint(String fp) {
    final groups = <String>[];
    for (var i = 0; i < fp.length; i += 4) {
      groups.add(fp.substring(i, i + 4 > fp.length ? fp.length : i + 4));
    }
    return groups.join(' ');
  }

  @override
  Future<void> saveTrusted(TrustedPeer p) async {
    final list = await trustedPeers();
    list.removeWhere((t) => t.fingerprint == p.fingerprint);
    list.add(p);
    await _storage.write(key: _kTrusted, value: _encodeTrusted(list));
  }

  @override
  Future<List<TrustedPeer>> trustedPeers() async {
    final raw = await _storage.read(key: _kTrusted);
    if (raw == null) return [];
    try {
      return (jsonDecode(raw) as List).map((e) {
        final j = e as Map<String, dynamic>;
        return TrustedPeer(
          fingerprint: j['fp'] as String,
          name: j['name'] as String,
          ed25519Public: base64Decode(j['edPub'] as String),
          x25519Public: base64Decode(j['xPub'] as String),
          lastSeen: DateTime.tryParse(j['seen'] as String? ?? ''),
        );
      }).toList();
    } catch (_) {
      return [];
    }
  }

  String _encodeTrusted(List<TrustedPeer> list) => jsonEncode(list
      .map((p) => {
            'fp': p.fingerprint,
            'name': p.name,
            'edPub': base64Encode(p.ed25519Public),
            'xPub': base64Encode(p.x25519Public),
            'seen': p.lastSeen.toIso8601String(),
          })
      .toList());

  @override
  Future<void> removeTrusted(String fingerprint) async {
    final list = await trustedPeers();
    list.removeWhere((t) => t.fingerprint == fingerprint);
    await _storage.write(key: _kTrusted, value: _encodeTrusted(list));
  }

  @override
  Future<Uint8List> vaultMasterKey() async {
    final raw = await _storage.read(key: _kVaultKey);
    if (raw != null) return base64Decode(raw);
    final key = await AesGcm.with256bits().newSecretKey();
    final bytes = await key.extractBytes();
    await _storage.write(key: _kVaultKey, value: base64Encode(bytes));
    return Uint8List.fromList(bytes);
  }

  @override
  Future<String> deviceName() async {
    final raw = await _storage.read(key: _kDeviceName);
    if (raw != null && raw.isNotEmpty) return raw;
    return 'DynamicIsland 设备';
  }

  @override
  Future<void> setDeviceName(String name) async {
    await _storage.write(key: _kDeviceName, value: name);
  }
}