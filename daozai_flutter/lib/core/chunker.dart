//! 分块与校验 —— 纯逻辑：分块计划、续传偏移、SHA-256 流式哈希（可单测）。
//! T1 用整文件传输+校验；offset 逻辑预留，T2 接断点续传。
import 'dart:convert';
import 'package:crypto/crypto.dart';

const int defaultChunkSize = 256 * 1024;

/// 分块计划：返回每块 (offset, length)。恰好整除不产生空块；余 1 字节也成块。
List<({int offset, int length})> chunkPlan(int totalSize, [int chunkSize = defaultChunkSize]) {
  if (totalSize <= 0) return [];
  final out = <({int offset, int length})>[];
  var off = 0;
  while (off < totalSize) {
    final len = (totalSize - off) < chunkSize ? totalSize - off : chunkSize;
    out.add((offset: off, length: len));
    off += len;
  }
  return out;
}

/// 续传起点：已收字节数即下一块 offset（T2 用）。
int resumeOffset(int receivedBytes) => receivedBytes;

/// 流式 SHA-256：喂块 → 最终 hex。收纳与拉取校验共用。
/// 用 crypto 的 ByteConversionSink 链，避免额外依赖 convert 包。
class Sha256Sink {
  final _out = _DigestCapture();
  late final ByteConversionSink _input = sha256.startChunkedConversion(_out);
  int _bytes = 0;
  bool _closed = false;

  void add(List<int> chunk) {
    _input.add(chunk);
    _bytes += chunk.length;
  }

  int get bytes => _bytes;

  String digest() {
    if (!_closed) {
      _input.close();
      _closed = true;
    }
    return _out.value!.toString();
  }
}

class _DigestCapture implements Sink<Digest> {
  Digest? value;
  @override
  void add(Digest data) => value = data;
  @override
  void close() {}
}

/// 便捷：整段字节哈希（测试/小文件用）。
String sha256Hex(List<int> bytes) => sha256.convert(bytes).toString();

/// 校验结果：长度与哈希都必须一致才算完整文件。
bool verifyBlob(String expectedSha256, int expectedSize, String actualSha256, int actualSize) =>
    expectedSize == actualSize && expectedSha256 == actualSha256;

/// 文件名安全化：去掉路径分隔符与控制字符（防目录穿越，导出层用）。
String sanitizeRelPath(String relPath) {
  final parts = relPath
      .replaceAll('\\', '/')
      .split('/')
      .where((p) => p.isNotEmpty && p != '.' && p != '..')
      .map((p) => p.replaceAll(RegExp(r'[\x00-\x1f]'), '').trim())
      .where((p) => p.isNotEmpty);
  return parts.join('/');
}

/// JSON 行辅助（索引增量载荷用）。
String jsonLine(Object o) => jsonEncode(o);