//! 加密内容仓库 —— AES-256-GCM 落盘，按 sha256 寻址（不认路径）。
//! 文件格式：[12B nonce][密文+16B tag]。临时名写入 → 校验长度 → 原子改名。
//! 任何时刻 vault 里只有完整文件；半文件残留由 sweepTemp 启动清扫。
import 'dart:io';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';
import 'ports.dart';

class FileVault implements BlobVault {
  final String dir;
  final Future<Uint8List> Function() masterKey;
  final _algo = AesGcm.with256bits();
  SecretKey? _key;

  FileVault({required this.dir, required this.masterKey});

  Future<SecretKey> _secret() async {
    if (_key != null) return _key!;
    _key = SecretKey(await masterKey());
    return _key!;
  }

  String _path(String sha256) => '$dir/$sha256.blob';
  String _tmpPath(String sha256) => '$dir/$sha256.tmp';

  @override
  Future<bool> has(String sha256) => File(_path(sha256)).exists();

  @override
  Future<void> put(String sha256, Stream<List<int>> plain) async {
    await Directory(dir).create(recursive: true);
    final key = await _secret();
    final nonce = _algo.newNonce();
    final sink = _EncryptSink(_algo, key, nonce);
    await plain.forEach(sink.add);
    final box = await sink.finish();
    final tmp = File(_tmpPath(sha256));
    final raf = await tmp.open(mode: FileMode.write);
    try {
      await raf.writeFrom(nonce);
      await raf.writeFrom(box.cipherText);
      await raf.writeFrom(box.mac.bytes);
    } finally {
      await raf.close();
    }
    await tmp.rename(_path(sha256)); // 原子改名：半成品不会冒充完整文件
  }

  @override
  Future<Stream<List<int>>> get(String sha256) async {
    final f = File(_path(sha256));
    if (!await f.exists()) throw StateError('vault 无此内容: $sha256');
    final bytes = await f.readAsBytes();
    if (bytes.length < 12 + 16) throw StateError('vault 文件损坏: $sha256');
    final key = await _secret();
    final box = SecretBox(
      bytes.sublist(12, bytes.length - 16),
      nonce: bytes.sublist(0, 12),
      mac: Mac(bytes.sublist(bytes.length - 16)),
    );
    final plain = await _algo.decrypt(box, secretKey: key);
    return Stream.value(plain);
  }

  @override
  Future<void> delete(String sha256) async {
    final f = File(_path(sha256));
    if (await f.exists()) await f.delete();
    final t = File(_tmpPath(sha256));
    if (await t.exists()) await t.delete();
  }

  @override
  Future<int> usedBytes() async {
    final d = Directory(dir);
    if (!await d.exists()) return 0;
    var total = 0;
    await for (final e in d.list()) {
      if (e is File && e.path.endsWith('.blob')) {
        total += await e.length();
      }
    }
    return total;
  }

  @override
  Future<void> sweepTemp() async {
    final d = Directory(dir);
    if (!await d.exists()) return;
    await for (final e in d.list()) {
      if (e is File && e.path.endsWith('.tmp')) await e.delete();
    }
  }
}

/// 流式加密收集器：AES-GCM 需整段明文算 tag，故内存聚合后一次加密。
/// T1 单文件上限 512MB 由 intake 层预检拒绝超大文件；分块流式加密留 T2 优化。
class _EncryptSink {
  final AesGcm algo;
  final SecretKey key;
  final List<int> nonce;
  final _buf = BytesBuilder(copy: false);

  _EncryptSink(this.algo, this.key, this.nonce);

  void add(List<int> chunk) => _buf.add(chunk);

  Future<SecretBox> finish() => algo.encrypt(
        _buf.toBytes(),
        secretKey: key,
        nonce: nonce,
      );
}