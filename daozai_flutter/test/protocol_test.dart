//! 协议回归测试 —— 帧编解码往返/任意切分/截断与超长拒收/载荷契约。
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:dynamic_island/core/protocol.dart';

void main() {
  test('帧编码→解析往返（含空载荷）', () {
    final f = Frame(MsgType.pingPong, Uint8List(0));
    final p = FrameParser();
    final out = p.feed(f.encode());
    expect(out.length, 1);
    expect(out.single.type, MsgType.pingPong);
    expect(out.single.payload, isEmpty);
  });

  test('任意切分喂入：半帧等待，补齐后产出；多帧一次产出', () {
    final a = Frame.fromJson(MsgType.hello, helloPayload(identityPub: 'a', ephemeralPub: 'b'));
    final b = Frame.fromJson(MsgType.authReady, authPayload('Mac', 'fp'));
    final bytes = Uint8List.fromList([...a.encode(), ...b.encode()]);
    final p = FrameParser();
    expect(p.feed(bytes.sublist(0, 3)), isEmpty);
    expect(p.feed(bytes.sublist(3, 7)), isEmpty);
    final out = p.feed(bytes.sublist(7));
    expect(out.length, 2);
    expect(out[0].json()['id'], 'a');
    expect(out[1].json()['name'], 'Mac');
  });

  test('超长帧拒收（FormatException）', () {
    final bad = Uint8List(5);
    bad[0] = MsgType.blobChunk;
    ByteData.sublistView(bad, 1, 5).setUint32(0, maxFrameLen + 1, Endian.big);
    expect(() => FrameParser().feed(bad), throwsFormatException);
  });

  test('hello 载荷：版本/公钥/token 可选', () {
    final j = helloPayload(identityPub: 'id', ephemeralPub: 'ep', pairToken: 'tk');
    expect(j['v'], protocolVersion);
    expect(j['token'], 'tk');
    expect(helloPayload(identityPub: 'id', ephemeralPub: 'ep').containsKey('token'), isFalse);
  });

  test('blobChunk 二进制载荷往返（offset 大数不失真）', () {
    final chunk = List<int>.generate(300, (i) => i % 256);
    final payload = blobChunkPayload('abc123', 1 << 40, chunk);
    final r = parseBlobChunk(payload);
    expect(r.sha256, 'abc123');
    expect(r.offset, 1 << 40);
    expect(r.chunk, chunk);
  });

  test('indexDelta/blobRequest/blobDone/error 载荷契约', () {
    expect(indexSyncRequestPayload(99)['wm'], 99);
    expect(blobRequestPayload('s', 5)['off'], 5);
    expect(blobDonePayload('s', 10)['total'], 10);
    expect(errorPayload('E1', '原因')['reason'], '原因');
    expect(pairConfirmPayload(true, 'B')['ok'], isTrue);
    expect(keyexPayload('sig')['sig'], 'sig');
  });
}