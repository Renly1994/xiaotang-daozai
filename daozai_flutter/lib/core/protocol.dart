//! 线缆协议 —— 纯逻辑：帧编解码、消息类型、载荷序列化（无平台依赖，可单测）。
//! 帧格式 [1B type][4B len][payload]；payload 的 AEAD 加密在 lan_transport 层。
import 'dart:convert';
import 'dart:typed_data';

const int protocolVersion = 1;

/// 帧长度上限（16MB）：防恶意/损坏帧撑爆内存。blobChunk 默认 256KB，远小于此。
const int maxFrameLen = 16 * 1024 * 1024;

class MsgType {
  MsgType._();
  static const int hello = 0x01;
  static const int pairConfirm = 0x02;
  static const int indexDelta = 0x03;
  static const int indexSyncRequest = 0x04;
  static const int blobRequest = 0x05;
  static const int blobChunk = 0x06;
  static const int blobDone = 0x07;
  static const int ackError = 0x08;
  static const int pingPong = 0x09;
  static const int keyex = 0x0A;
  static const int authReady = 0x0B;
}

class Frame {
  final int type;
  final Uint8List payload;
  Frame(this.type, this.payload);

  /// 编码为线缆字节（明文头 + payload；加密由传输层负责）。
  Uint8List encode() {
    final out = Uint8List(5 + payload.length);
    out[0] = type;
    final bd = ByteData.sublistView(out, 1, 5);
    bd.setUint32(0, payload.length, Endian.big);
    out.setRange(5, out.length, payload);
    return out;
  }

  Map<String, dynamic> json() =>
      jsonDecode(utf8.decode(payload)) as Map<String, dynamic>;

  static Frame fromJson(int type, Map<String, dynamic> j) =>
      Frame(type, Uint8List.fromList(utf8.encode(jsonEncode(j))));
}

/// 流式帧解析器：喂入任意切分的字节，产出完整帧。
/// 截断/超长帧 → 抛 FormatException（调用方断开连接）。
class FrameParser {
  final _buf = BytesBuilder(copy: false);

  List<Frame> feed(List<int> bytes) {
    _buf.add(bytes);
    final out = <Frame>[];
    var data = _buf.toBytes();
    var offset = 0;
    while (data.length - offset >= 5) {
      final bd = ByteData.sublistView(data, offset, offset + 5);
      final len = bd.getUint32(1, Endian.big);
      if (len > maxFrameLen) {
        throw FormatException('帧超长: $len');
      }
      if (data.length - offset - 5 < len) break; // 半帧，等更多字节
      out.add(Frame(data[offset], Uint8List.sublistView(data, offset + 5, offset + 5 + len)));
      offset += 5 + len;
    }
    _buf.clear();
    if (offset < data.length) _buf.add(Uint8List.sublistView(data, offset));
    return out;
  }
}

// ---- 载荷构造/解析（JSON；字段名即协议契约，改动需升版本）----

Map<String, dynamic> helloPayload({
  required String identityPub, // base64 Ed25519 公钥
  required String ephemeralPub, // base64 X25519 临时公钥
  String? pairToken,
}) =>
    {
      'v': protocolVersion,
      'id': identityPub,
      'ep': ephemeralPub,
      if (pairToken != null) 'token': pairToken,
    };

Map<String, dynamic> keyexPayload(String transcriptSig) => {'sig': transcriptSig};

Map<String, dynamic> authPayload(String deviceName, String fingerprint) =>
    {'name': deviceName, 'fp': fingerprint};

Map<String, dynamic> pairConfirmPayload(bool confirmed, String deviceName) =>
    {'ok': confirmed, 'name': deviceName};

Map<String, dynamic> indexSyncRequestPayload(int watermark) => {'wm': watermark};

Map<String, dynamic> blobRequestPayload(String sha256, int offset) =>
    {'sha': sha256, 'off': offset};

Map<String, dynamic> blobDonePayload(String sha256, int totalSize) =>
    {'sha': sha256, 'total': totalSize};

Map<String, dynamic> errorPayload(String code, String reason) =>
    {'code': code, 'reason': reason};

/// blobChunk 载荷：1B shaLen + sha + 8B offset + 原始块字节（不走 JSON，避免 base64 膨胀）。
Uint8List blobChunkPayload(String sha256, int offset, List<int> chunk) {
  final shaBytes = utf8.encode(sha256);
  final out = Uint8List(1 + shaBytes.length + 8 + chunk.length);
  out[0] = shaBytes.length;
  out.setRange(1, 1 + shaBytes.length, shaBytes);
  final bd = ByteData.sublistView(out, 1 + shaBytes.length, 9 + shaBytes.length);
  bd.setUint64(0, offset, Endian.big);
  out.setRange(9 + shaBytes.length, out.length, chunk);
  return out;
}

({String sha256, int offset, Uint8List chunk}) parseBlobChunk(Uint8List payload) {
  final shaLen = payload[0];
  final sha = utf8.decode(payload.sublist(1, 1 + shaLen));
  final bd = ByteData.sublistView(payload, 1 + shaLen, 9 + shaLen);
  final off = bd.getUint64(0, Endian.big);
  return (sha256: sha, offset: off, chunk: Uint8List.sublistView(payload, 9 + shaLen));
}