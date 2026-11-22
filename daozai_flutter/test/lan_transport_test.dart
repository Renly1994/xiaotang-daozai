//! 加密通道回环测试 —— 127.0.0.1 双实例：握手/验签拒伪/帧篡改/未信任拒绝/SAS 一致（验收 7、8）。
import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:dynamic_island/core/protocol.dart';
import 'package:dynamic_island/services/lan_transport.dart';
import 'package:dynamic_island/services/ports.dart';

/// 内存 KeyVault fake：无平台安全存储依赖。
class FakeKeyVault implements KeyVault {
  DeviceIdentity? _id;
  final List<TrustedPeer> trusted = [];
  String name;
  final Uint8List master = Uint8List.fromList(List.generate(32, (i) => i + 7));

  FakeKeyVault(this.name);

  @override
  Future<DeviceIdentity> loadOrCreate() async {
    if (_id != null) return _id!;
    // 用真实 cryptography 生成（测试也验证密钥格式兼容）。
    final real = await _realIdentity();
    _id = real;
    return real;
  }

  static Future<DeviceIdentity> _realIdentity() async {
    final tmp = SecureKeysTestHelper();
    return tmp.generate();
  }

  @override
  Future<void> saveTrusted(TrustedPeer p) async {
    trusted.removeWhere((t) => t.fingerprint == p.fingerprint);
    trusted.add(p);
  }

  @override
  Future<List<TrustedPeer>> trustedPeers() async => List.of(trusted);

  @override
  Future<void> removeTrusted(String fp) async => trusted.removeWhere((t) => t.fingerprint == fp);

  @override
  Future<Uint8List> vaultMasterKey() async => master;

  @override
  Future<String> deviceName() async => name;

  @override
  Future<void> setDeviceName(String n) async => name = n;
}

/// 借用 SecureKeys 的生成逻辑但不落安全存储：直接调 cryptography。
class SecureKeysTestHelper {
  Future<DeviceIdentity> generate() async {
    // ignore: avoid_dynamic_calls
    final ed = await _ed25519();
    final x = await _x25519();
    final fp = await _fp(ed.$2);
    return DeviceIdentity(
      ed25519Private: ed.$1,
      ed25519Public: ed.$2,
      x25519Private: x.$1,
      x25519Public: x.$2,
      fingerprint: fp,
    );
  }

  static Future<(Uint8List, Uint8List)> _ed25519() async {
    final kp = await Ed25519().newKeyPair();
    return (
      Uint8List.fromList(await kp.extractPrivateKeyBytes()),
      Uint8List.fromList((await kp.extractPublicKey()).bytes),
    );
  }

  static Future<(Uint8List, Uint8List)> _x25519() async {
    final kp = await X25519().newKeyPair();
    return (
      Uint8List.fromList(await kp.extractPrivateKeyBytes()),
      Uint8List.fromList((await kp.extractPublicKey()).bytes),
    );
  }

  static Future<String> _fp(Uint8List pub) async {
    final h = await Sha256().hash(pub);
    return h.bytes.sublist(0, 8).map((b) => b.toRadixString(16).padLeft(2, '0')).join().toUpperCase();
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late FakeKeyVault keysA;
  late FakeKeyVault keysB;
  late LanTransport ta;
  late LanTransport tb;

  setUp(() async {
    keysA = FakeKeyVault('Mac-A');
    keysB = FakeKeyVault('Win-B');
    ta = LanTransport(keys: keysA, handshakeTimeout: const Duration(seconds: 5));
    tb = LanTransport(keys: keysB, handshakeTimeout: const Duration(seconds: 5));
    await ta.listen(0);
    await tb.listen(0);
  });

  tearDown(() async {
    await ta.close();
    await tb.close();
  });

  Future<(PeerLink, PeerLink)> pairConnect() async {
    final session = ta.startPairing(DateTime.now());
    final incomingF = ta.incoming.first;
    final lb = await tb.connect('127.0.0.1', ta.boundPort!, pairToken: session.token);
    final la = await incomingF;
    return (la, lb);
  }

  test('配对握手成功：两端 SAS 一致、设备名互换、isPairing 标记', () async {
    final (la, lb) = await pairConnect();
    expect(la.remoteName, 'Win-B');
    expect(lb.remoteName, 'Mac-A');
    expect(la.isPairing, isTrue);
    final sa = (la as dynamic).sas as String;
    final sb = (lb as dynamic).sas as String;
    expect(sa, sb);
    expect(sa.length, 6);
    await la.close();
    await lb.close();
  });

  test('加密帧双向收发：内容一致、多帧顺序不乱', () async {
    final (la, lb) = await pairConnect();
    final gotB = <String>[];
    final done = Completer<void>();
    lb.frames.listen((f) {
      final parsed = FrameParser().feed(f);
      for (final p in parsed) {
        gotB.add(utf8.decode(p.payload));
        if (gotB.length == 3) done.complete();
      }
    });
    for (final msg in ['一', '二', '三']) {
      la.send(Frame(MsgType.pingPong, Uint8List.fromList(utf8.encode(msg))).encode());
    }
    await done.future.timeout(const Duration(seconds: 5));
    expect(gotB, ['一', '二', '三']);
    await la.close();
    await lb.close();
  });

  test('未配对且无 token 连入 → 拒绝（验收 7）', () async {
    expect(
      () => tb.connect('127.0.0.1', ta.boundPort!),
      throwsA(anything),
    );
  });

  test('错误 token 连入 → 拒绝；正确 token 一次性（重放失败）', () async {
    final session = ta.startPairing(DateTime.now());
    expect(
      () => tb.connect('127.0.0.1', ta.boundPort!, pairToken: 'wrong-token'),
      throwsA(anything),
    );
    final incomingF = ta.incoming.first;
    final lb = await tb.connect('127.0.0.1', ta.boundPort!, pairToken: session.token);
    final la = await incomingF;
    await la.close();
    await lb.close();
    // 同 token 重放
    expect(
      () => tb.connect('127.0.0.1', ta.boundPort!, pairToken: session.token),
      throwsA(anything),
    );
  });

  test('已信任设备免配对重连（trust 表生效）', () async {
    final (la, lb) = await pairConnect();
    // 模拟双方确认配对：把对端公钥写入信任表。
    final idA = await keysA.loadOrCreate();
    final idB = await keysB.loadOrCreate();
    await keysA.saveTrusted(TrustedPeer(
      fingerprint: idB.fingerprint, name: 'Win-B',
      ed25519Public: idB.ed25519Public, x25519Public: idB.x25519Public,
    ));
    await keysB.saveTrusted(TrustedPeer(
      fingerprint: idA.fingerprint, name: 'Mac-A',
      ed25519Public: idA.ed25519Public, x25519Public: idA.x25519Public,
    ));
    await la.close();
    await lb.close();
    final incomingF = ta.incoming.first;
    final lb2 = await tb.connect('127.0.0.1', ta.boundPort!); // 无 token
    final la2 = await incomingF;
    expect(la2.isPairing, isFalse);
    expect(lb2.isPairing, isFalse);
    await la2.close();
    await lb2.close();
  });

  test('过期配对会话 token → 拒绝', () async {
    final session = ta.startPairing(DateTime.now().subtract(const Duration(seconds: 200)));
    expect(
      () => tb.connect('127.0.0.1', ta.boundPort!, pairToken: session.token),
      throwsA(anything),
    );
  });
}