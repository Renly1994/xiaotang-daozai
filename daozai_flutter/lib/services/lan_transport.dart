//! 局域网加密通道 —— dart:io TCP + 四消息握手 + XChaCha20-Poly1305 帧加密。
//! 握手（design §2.4）：hello(明文) → keyex(明文,transcript 签名) → auth(加密) → ready(加密)。
//! 双 ECDH（临时×临时 + 静态×静态）→ HKDF-SHA256 派生双向会话密钥；Ed25519 验签防中间人。
//! 未信任且无有效配对 token 的连入一律拒绝（R1.6）。
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';
import '../core/pairing.dart';
import '../core/protocol.dart';
import 'ports.dart';

/// 字节流上的顺序读取器：握手期按需取「一个明文帧」或「一个加密帧」。
/// 持有唯一 socket 订阅；握手结束后 pause 并把缓冲+订阅移交 _Link（不二次 listen）。
class _ByteReader {
  final StreamSubscription<Uint8List> sub;
  final _buf = BytesBuilder(copy: false);
  final _waiters = <Completer<void>>[];
  bool _done = false;
  Object? _error;

  _ByteReader(Stream<Uint8List> stream) : sub = stream.listen(null) {
    sub.onData(_onBytes);
    sub.onError((Object e) {
      _error = e;
      _wake();
    });
    sub.onDone(() {
      _done = true;
      _wake();
    });
  }

  void _onBytes(Uint8List bytes) {
    _buf.add(bytes);
    _wake();
  }

  void _wake() {
    for (final w in _waiters) {
      if (!w.isCompleted) w.complete();
    }
    _waiters.clear();
  }

  Future<Uint8List> _take(int n) async {
    while (_buf.length < n) {
      if (_error != null) throw _error!;
      if (_done) throw StateError('连接断开');
      final w = Completer<void>();
      _waiters.add(w);
      await w.future;
    }
    final data = _buf.toBytes();
    final out = Uint8List.sublistView(data, 0, n);
    _buf.clear();
    if (data.length > n) _buf.add(Uint8List.sublistView(data, n));
    return out;
  }

  /// 明文帧：[1B type][4B len][payload]。
  Future<Frame> nextPlain(Duration timeout) async {
    final head = await _take(5).timeout(timeout);
    final len = ByteData.sublistView(head, 1, 5).getUint32(0, Endian.big);
    if (len > maxFrameLen) throw FormatException('帧超长');
    final payload = len == 0 ? Uint8List(0) : await _take(len).timeout(timeout);
    return Frame(head[0], payload);
  }

  /// 加密帧：[5B 明文头][密文 len][16B tag] → (head, cipher, mac)。
  Future<(Uint8List, Uint8List, Uint8List)> nextEncrypted(Duration timeout) async {
    final head = await _take(5).timeout(timeout);
    final len = ByteData.sublistView(head, 1, 5).getUint32(0, Endian.big);
    if (len > maxFrameLen) throw FormatException('帧超长');
    final body = await _take(len + 16).timeout(timeout);
    return (head, Uint8List.sublistView(body, 0, len), Uint8List.sublistView(body, len));
  }

  /// 握手结束：暂停订阅，取出剩余缓冲（_Link 接管后恢复）。
  Uint8List handoff() {
    sub.pause();
    final d = _buf.toBytes();
    _buf.clear();
    return d;
  }

  Future<void> cancel() => sub.cancel();
}

class LanTransport implements PeerTransport {
  final KeyVault keys;
  final Duration handshakeTimeout;
  ServerSocket? _server;
  final _incoming = StreamController<PeerLink>.broadcast();
  final _algo = Xchacha20.poly1305Aead();
  final _ed = Ed25519();
  final _x = X25519();
  final _hkdf = Hkdf(hmac: Hmac.sha256(), outputLength: 64);

  /// 配对会话表：token → session（advertising 端持有）。
  final Map<String, PairingSession> pairSessions = {};

  LanTransport({required this.keys, this.handshakeTimeout = const Duration(seconds: 10)});

  @override
  Stream<PeerLink> get incoming => _incoming.stream;

  @override
  Future<void> listen(int port) async {
    _server = await ServerSocket.bind(InternetAddress.anyIPv4, port);
    _server!.listen((socket) async {
      try {
        final link = await _handshake(socket, initiator: false);
        if (!_incoming.isClosed) _incoming.add(link);
      } catch (_) {
        socket.destroy();
      }
    });
  }

  @override
  int? get boundPort => _server?.port;

  @override
  Future<PeerLink> connect(String host, int port, {String? pairToken}) async {
    final socket = await Socket.connect(host, port, timeout: handshakeTimeout);
    return _handshake(socket, initiator: true, pairToken: pairToken);
  }

  @override
  Future<void> close() async {
    await _server?.close();
    _server = null;
    if (!_incoming.isClosed) await _incoming.close();
  }

  /// 发起配对广播：生成 token+session，返回给 UI 展示（二维码/短码通道）。
  @override
  PairingSession startPairing(DateTime now) {
    final token = generatePairToken();
    final s = PairingSession(token: token, createdAt: now);
    s.state = PairState.advertising;
    pairSessions[token] = s;
    return s;
  }

  Future<PeerLink> _handshake(Socket socket, {required bool initiator, String? pairToken}) async {
    final identity = await keys.loadOrCreate();
    final trusted = await keys.trustedPeers();
    final name = await keys.deviceName();
    final eph = await _x.newKeyPair();
    final ephPub = (await eph.extractPublicKey()).bytes;
    final reader = _ByteReader(socket);

    try {
      // ---- hello（明文）----
      final myHello = Frame.fromJson(
        MsgType.hello,
        {
          ...helloPayload(
            identityPub: base64Encode(identity.ed25519Public),
            ephemeralPub: base64Encode(ephPub),
            pairToken: pairToken,
          ),
          'xPub': base64Encode(identity.x25519Public),
        },
      );
      Frame peerHello;
      if (initiator) {
        socket.add(myHello.encode());
        peerHello = await reader.nextPlain(handshakeTimeout);
      } else {
        peerHello = await reader.nextPlain(handshakeTimeout);
        socket.add(myHello.encode());
      }
      if (peerHello.type != MsgType.hello) throw StateError('期待 hello');
      final hj = peerHello.json();
      if (hj['v'] != protocolVersion) throw StateError('协议版本不匹配');
      final peerIdPub = Uint8List.fromList(base64Decode(hj['id'] as String));
      final peerEphPub = Uint8List.fromList(base64Decode(hj['ep'] as String));
      final peerFp = await _fingerprint(peerIdPub);
      final presentedToken = hj['token'] as String?;

      // 信任判定：已信任 → 正常握手；否则必须走配对（R1.6）。
      TrustedPeer? trust;
      for (final t in trusted) {
        if (t.fingerprint == peerFp) trust = t;
      }
      PairingSession? session;
      var isPairing = false;
      if (trust == null) {
        isPairing = true;
        if (!initiator) {
          if (presentedToken == null) throw StateError('未配对设备连入拒绝');
          session = pairSessions[presentedToken];
          if (session == null || !session.consumeToken(presentedToken, DateTime.now())) {
            throw StateError('配对 token 无效');
          }
        } else {
          if (pairToken == null) throw StateError('未配对设备需携带配对码');
        }
      } else {
        if (!_bytesEqual(trust.ed25519Public, peerIdPub)) {
          throw StateError('身份公钥与信任记录不符');
        }
      }

      // ---- keyex（明文，transcript 签名；transcript=双方 hello 线缆字节按固定序拼接）----
      // 固定序：initiator hello 在前，responder hello 在后（两端一致）。
      final transcript = initiator
          ? Uint8List.fromList([...myHello.encode(), ...peerHello.encode()])
          : Uint8List.fromList([...peerHello.encode(), ...myHello.encode()]);
      final myKeyPair = SimpleKeyPairData(
        identity.ed25519Private,
        publicKey: SimplePublicKey(identity.ed25519Public, type: KeyPairType.ed25519),
        type: KeyPairType.ed25519,
      );
      final mySig = await _ed.sign(transcript, keyPair: myKeyPair);
      final myKeyex = Frame.fromJson(MsgType.keyex, keyexPayload(base64Encode(mySig.bytes)));
      Frame peerKeyex;
      if (initiator) {
        socket.add(myKeyex.encode());
        peerKeyex = await reader.nextPlain(handshakeTimeout);
      } else {
        peerKeyex = await reader.nextPlain(handshakeTimeout);
        socket.add(myKeyex.encode());
      }
      if (peerKeyex.type != MsgType.keyex) throw StateError('期待 keyex');
      final peerSig = base64Decode(peerKeyex.json()['sig'] as String);
      final sigOk = await _ed.verify(
        transcript,
        signature: Signature(
          peerSig,
          publicKey: SimplePublicKey(peerIdPub, type: KeyPairType.ed25519),
        ),
      );
      if (!sigOk) throw StateError('验签失败（可能中间人）');

      // ---- 派生会话密钥 ----
      // 临时×临时：前向保密 + SAS 来源。
      final ephShared = await _x.sharedSecretKey(
        keyPair: eph,
        remotePublicKey: SimplePublicKey(peerEphPub, type: KeyPairType.x25519),
      );
      final ephBytes = await ephShared.extractBytes();
      // hello 同时声明静态 X25519 公钥（设计内字段扩展）：配对与重连都走完整双 ECDH。
      final peerStaticXPub = Uint8List.fromList(base64Decode(hj['xPub'] as String));
      final myStatic = SimpleKeyPairData(
        identity.x25519Private,
        publicKey: SimplePublicKey(identity.x25519Public, type: KeyPairType.x25519),
        type: KeyPairType.x25519,
      );
      // 已信任设备：静态公钥必须与信任表一致（防 hello 伪造）。
      if (trust != null && !_bytesEqual(trust.x25519Public, peerStaticXPub)) {
        throw StateError('静态公钥与信任记录不符');
      }
      final staticShared = await _x.sharedSecretKey(
        keyPair: myStatic,
        remotePublicKey: SimplePublicKey(peerStaticXPub, type: KeyPairType.x25519),
      );
      final staticBytes = await staticShared.extractBytes();
      final ikm = Uint8List.fromList([...ephBytes, ...staticBytes]);
      // 方向密钥：两端派生同一 64B，initiator 发用前半收用后半，responder 相反。
      final derived = await _hkdf.deriveKey(
        secretKey: SecretKey(ikm),
        nonce: utf8.encode('daozai-v1-direction'),
        info: utf8.encode('daozai-session'),
      );
      final dBytes = await derived.extractBytes();
      final keyA = SecretKey(dBytes.sublist(0, 32));
      final keyB = SecretKey(dBytes.sublist(32, 64));
      final sendKey = initiator ? keyA : keyB;
      final recvKey = initiator ? keyB : keyA;
      final sasHash = await Sha256().hash(ephBytes);
      final sas = sasFromSharedSecret(Uint8List.fromList(sasHash.bytes));

      // ---- 加密帧收发工具 ----
      var sendCounter = 0;
      var recvCounter = 0;
      Future<void> sendEnc(Frame f) async {
        final encoded = f.encode();
        final head = encoded.sublist(0, 5);
        final box = await _algo.encrypt(
          encoded.sublist(5),
          secretKey: sendKey,
          nonce: _counterNonce(sendCounter),
          aad: head,
        );
        final out = BytesBuilder(copy: false);
        out.add(head);
        out.add(box.cipherText);
        out.add(box.mac.bytes);
        socket.add(out.toBytes());
        sendCounter++;
      }

      Future<Frame> nextEnc() async {
        final (head, cipher, mac) = await reader.nextEncrypted(handshakeTimeout);
        final plain = await _algo.decrypt(
          SecretBox(cipher, nonce: _counterNonce(recvCounter), mac: Mac(mac)),
          secretKey: recvKey,
          aad: head,
        );
        recvCounter++;
        return Frame(head[0], Uint8List.fromList(plain));
      }

      // ---- auth（加密）：设备名 + 指纹 + 静态公钥 ----
      final myAuth = Frame.fromJson(MsgType.authReady, {
        ...authPayload(name, identity.fingerprint),
        'xPub': base64Encode(identity.x25519Public),
        'phase': 'auth',
      });
      Frame peerAuth;
      if (initiator) {
        await sendEnc(myAuth);
        peerAuth = await nextEnc();
      } else {
        peerAuth = await nextEnc();
        await sendEnc(myAuth);
      }
      final aj = peerAuth.json();
      if (aj['fp'] != peerFp) throw StateError('指纹不一致（可能中间人）');
      final peerName = aj['name'] as String? ?? '未知设备';
      final peerStaticX = Uint8List.fromList(base64Decode(aj['xPub'] as String));

      if (isPairing) {
        session?.onKeyexDone(sas, peerName, peerFp);
      }

      // ---- ready（加密）----
      final myReady = Frame.fromJson(MsgType.authReady, {'phase': 'ready'});
      if (initiator) {
        await sendEnc(myReady);
        final r = await nextEnc();
        if (r.json()['phase'] != 'ready') throw StateError('期待 ready');
      } else {
        final r = await nextEnc();
        if (r.json()['phase'] != 'ready') throw StateError('期待 ready');
        await sendEnc(myReady);
      }

      // 握手完成：暂停订阅，剩余缓冲+订阅移交链路（不二次 listen socket）。
      final leftover = reader.handoff();
      return _Link(
        socket: socket,
        readerSub: reader.sub,
        algo: _algo,
        sendKey: sendKey,
        recvKey: recvKey,
        remoteFingerprint: peerFp,
        remoteName: peerName,
        isPairing: isPairing,
        session: session,
        sas: sas,
        peerIdentityPub: peerIdPub,
        peerStaticXPub: peerStaticX,
        leftover: leftover,
        sendCounter: sendCounter,
        recvCounter: recvCounter,
      );
    } catch (e) {
      socket.destroy();
      await reader.cancel();
      rethrow;
    }
  }

  Future<String> _fingerprint(Uint8List edPub) async {
    final h = await Sha256().hash(edPub);
    return h.bytes
        .sublist(0, 8)
        .map((b) => b.toRadixString(16).padLeft(2, '0'))
        .join()
        .toUpperCase();
  }

  static bool _bytesEqual(List<int> a, List<int> b) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (a[i] != b[i]) return false;
    }
    return true;
  }

  static Uint8List _counterNonce(int counter) {
    final n = Uint8List(24);
    ByteData.sublistView(n).setUint64(16, counter, Endian.little);
    return n;
  }
}

/// 加密链路：串行解密队列保证帧序；send 串行加密保证计数器不乱。
class _Link implements PeerLink {
  final Socket socket;
  final Xchacha20 algo;
  final SecretKey sendKey;
  final SecretKey recvKey;
  @override
  final String remoteFingerprint;
  @override
  final String remoteName;
  @override
  bool isPairing;
  final PairingSession? session;
  @override
  PairingSessionView? get pairingSession => session;
  @override
  final String sas;
  @override
  final Uint8List peerIdentityPub;
  @override
  final Uint8List peerStaticXPub;
  final _frames = StreamController<Uint8List>.broadcast();
  final _buf = BytesBuilder(copy: false);
  int _sendCounter;
  int _recvCounter;
  StreamSubscription<Uint8List>? _sub;
  bool _closed = false;
  bool _draining = false;
  Future<void> _sendChain = Future.value();

  _Link({
    required this.socket,
    required StreamSubscription<Uint8List> readerSub,
    required this.algo,
    required this.sendKey,
    required this.recvKey,
    required this.remoteFingerprint,
    required this.remoteName,
    required this.isPairing,
    required this.session,
    required this.sas,
    required this.peerIdentityPub,
    required this.peerStaticXPub,
    required Uint8List leftover,
    required int sendCounter,
    required int recvCounter,
  })  : _sendCounter = sendCounter,
        _recvCounter = recvCounter {
    if (leftover.isNotEmpty) _buf.add(leftover);
    _sub = readerSub;
    _sub!.onData((bytes) {
      _buf.add(bytes);
      _drain();
    });
    _sub!.onError((Object e) {
      if (!_closed) _frames.addError(e);
    });
    _sub!.onDone(() => close());
    _sub!.resume();
    _drain();
  }

  /// 串行解密：一次只处理一帧，完成后继续下一帧（计数器严格递增）。
  void _drain() {
    if (_draining || _closed) return;
    _draining = true;
    () async {
      try {
        while (!_closed) {
          final parsed = _tryParse();
          if (parsed == null) break;
          final (head, cipher, mac) = parsed;
          final plain = await algo.decrypt(
            SecretBox(cipher, nonce: LanTransport._counterNonce(_recvCounter), mac: Mac(mac)),
            secretKey: recvKey,
            aad: head,
          );
          _recvCounter++;
          if (_closed) break;
          final full = Uint8List(5 + plain.length);
          full.setRange(0, 5, head);
          full.setRange(5, full.length, plain);
          _frames.add(full);
        }
      } catch (_) {
        if (!_closed) {
          _frames.addError(StateError('帧解密失败（篡改或密钥不符）'));
          await close();
        }
      } finally {
        _draining = false;
      }
    }();
  }

  (Uint8List, Uint8List, Uint8List)? _tryParse() {
    final data = _buf.toBytes();
    if (data.length < 5) return null;
    final len = ByteData.sublistView(data, 1, 5).getUint32(0, Endian.big);
    if (len > maxFrameLen) throw FormatException('帧超长');
    final total = 5 + len + 16;
    if (data.length < total) return null;
    final head = Uint8List.sublistView(data, 0, 5);
    final cipher = Uint8List.sublistView(data, 5, 5 + len);
    final mac = Uint8List.sublistView(data, 5 + len, total);
    _buf.clear();
    if (data.length > total) _buf.add(Uint8List.sublistView(data, total));
    return (head, cipher, mac);
  }

  @override
  Stream<Uint8List> get frames => _frames.stream;

  @override
  void send(Uint8List frame) {
    if (_closed) return;
    // 串行化加密+发送：保证 nonce 计数器与线缆顺序一致。
    _sendChain = _sendChain.then((_) async {
      if (_closed) return;
      final head = frame.sublist(0, 5);
      final box = await algo.encrypt(
        frame.sublist(5),
        secretKey: sendKey,
        nonce: LanTransport._counterNonce(_sendCounter),
        aad: head,
      );
      final out = BytesBuilder(copy: false);
      out.add(head);
      out.add(box.cipherText);
      out.add(box.mac.bytes);
      socket.add(out.toBytes());
      _sendCounter++;
    });
  }

  @override
  Future<void> close() async {
    if (_closed) return;
    _closed = true;
    await _sub?.cancel();
    socket.destroy();
    if (!_frames.isClosed) await _frames.close();
  }
}