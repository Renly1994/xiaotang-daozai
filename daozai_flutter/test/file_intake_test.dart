//! 收纳管线回归测试 —— 原件不动/去重/超限/磁盘不足/文件夹与链接跳过/部分失败不中断。
//! 覆盖验收 1、3、4。
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';
import 'package:dynamic_island/services/download_export.dart';
import 'package:dynamic_island/services/file_intake.dart';
import 'package:dynamic_island/services/file_vault.dart';
import 'package:dynamic_island/services/ports.dart';
import 'package:dynamic_island/services/sqlite_store.dart';
import 'package:dynamic_island/services/sync_engine.dart';
import 'package:dynamic_island/state/sync_state.dart';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';

class _MemKeys implements KeyVault {
  DeviceIdentity? _id;
  final _master = Uint8List.fromList(List.generate(32, (i) => i + 3));
  @override
  Future<DeviceIdentity> loadOrCreate() async {
    if (_id != null) return _id!;
    final ed = await Ed25519().newKeyPair();
    final x = await X25519().newKeyPair();
    final pub = (await ed.extractPublicKey()).bytes;
    final h = await Sha256().hash(pub);
    _id = DeviceIdentity(
      ed25519Private: Uint8List.fromList(await ed.extractPrivateKeyBytes()),
      ed25519Public: Uint8List.fromList(pub),
      x25519Private: Uint8List.fromList(await x.extractPrivateKeyBytes()),
      x25519Public: Uint8List.fromList((await x.extractPublicKey()).bytes),
      fingerprint: h.bytes.sublist(0, 8).map((b) => b.toRadixString(16).padLeft(2, '0')).join().toUpperCase(),
    );
    return _id!;
  }
  @override
  Future<void> saveTrusted(TrustedPeer p) async {}
  @override
  Future<List<TrustedPeer>> trustedPeers() async => [];
  @override
  Future<void> removeTrusted(String fp) async {}
  @override
  Future<Uint8List> vaultMasterKey() async => _master;
  @override
  Future<String> deviceName() async => 'Test';
  @override
  Future<void> setDeviceName(String n) async {}
}

/// 无网络 transport fake：收纳测试不需要连接。
class _NullTransport implements PeerTransport {
  @override
  Future<void> listen(int port) async {}
  @override
  int? get boundPort => null;
  @override
  Future<PeerLink> connect(String host, int port, {String? pairToken}) async => throw StateError('no network');
  @override
  Stream<PeerLink> get incoming => const Stream.empty();
  @override
  PairingSessionView startPairing(DateTime now) => throw StateError('no pairing');
  @override
  Future<void> close() async {}
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  sqfliteFfiInit();

  var dbSeq = 0;
  late Directory tmp;
  late FileVault vault;
  late SyncEngine engine;
  late SyncState state;
  late ShelfIntake intake;

  setUp(() async {
    SharedPreferences.setMockInitialValues({});
    // ffi 内存库按路径共享单例：每测试用唯一名，防跨测试数据泄漏。
    dbSeq++;
    tmp = await Directory.systemTemp.createTemp('daozai_intake');
    final keys = _MemKeys();
    vault = FileVault(dir: '${tmp.path}/vault', masterKey: keys.vaultMasterKey);
    final db = await databaseFactoryFfi.openDatabase('file:memdb$dbSeq?mode=memory&cache=shared');
    final store = SqliteShelfStore.fromDatabase(db, masterKey: keys.vaultMasterKey);
    final download = DownloadExportImpl(vault: vault, defaultDirResolver: () async => '${tmp.path}/dl');
    state = SyncState();
    engine = SyncEngine(
      store: store, vault: vault, download: download, keys: keys,
      transport: _NullTransport(), events: state,
    );
    await engine.start();
    state.bind(engine);
    intake = ShelfIntake(vault: vault, engine: engine, state: state, freeDiskBytes: () async => 1 << 40);
  });

  tearDown(() async {
    await engine.dispose();
    state.dispose();
    if (await tmp.exists()) await tmp.delete(recursive: true);
  });

  Future<String> makeFile(String name, List<int> content) async {
    final f = File('${tmp.path}/src/$name');
    await f.parent.create(recursive: true);
    await f.writeAsBytes(content);
    return f.path;
  }

  test('收纳成功：原件不动、副本入库、索引可见、摘要正确（验收 1）', () async {
    final content = 'hello intake'.codeUnits;
    final p = await makeFile('a.txt', content);
    final r = await intake.intakePaths([p]);
    expect(r.summary.added, 1);
    expect(await File(p).readAsBytes(), content); // 原件仍在且内容不变
    expect(engine.entries.length, 1);
    expect(engine.entries.single.name, 'a.txt');
    expect(await vault.has(engine.entries.single.sha256), isTrue);
    expect(state.intakeSummary!.message, contains('已添加 1'));
  });

  test('重复拖入同内容 → 去重提示、存储不翻倍（验收 3）', () async {
    final p = await makeFile('a.txt', 'same'.codeUnits);
    await intake.intakePaths([p]);
    final used1 = await vault.usedBytes();
    final r = await intake.intakePaths([p]);
    expect(r.summary.duplicate, 1);
    expect(r.summary.added, 0);
    expect(await vault.usedBytes(), used1);
    expect(state.intakeSummary!.message, contains('已在置物架'));
  });

  test('超限拒绝且原因可见；同批其他文件不受影响（验收 4）', () async {
    final small = await makeFile('small.txt', 'ok'.codeUnits);
    final big = await makeFile('big.bin', List.filled(2048, 7));
    final limited = ShelfIntake(vault: vault, engine: engine, state: state, maxFileBytes: 1024, freeDiskBytes: () async => 1 << 40);
    final r = await limited.intakePaths([big, small]);
    expect(r.summary.failed, 1);
    expect(r.summary.added, 1);
    expect(r.summary.failReasons.single, contains('上限'));
    expect(state.notice, isNotNull);
  });

  test('磁盘不足 → 拒绝该文件，红条说明', () async {
    final p = await makeFile('x.txt', List.filled(4096, 1));
    final poor = ShelfIntake(vault: vault, engine: engine, state: state, freeDiskBytes: () async => 100);
    final r = await poor.intakePaths([p]);
    expect(r.summary.failed, 1);
    expect(r.summary.failReasons.single, contains('磁盘空间不足'));
    expect(engine.entries, isEmpty);
  });

  test('文件夹与符号链接跳过并计数（T1 文件夹提示下一版）', () async {
    final dir = Directory('${tmp.path}/src/folder');
    await dir.create(recursive: true);
    final target = await makeFile('target.txt', 't'.codeUnits);
    final link = Link('${tmp.path}/src/link.txt');
    await link.create(target);
    final r = await intake.intakePaths([dir.path, link.path]);
    expect(r.summary.skipped, 2);
    expect(r.summary.added, 0);
    expect(r.summary.failReasons.any((x) => x.contains('下一版')), isTrue);
    expect(r.summary.failReasons.any((x) => x.contains('符号链接')), isTrue);
  });

  test('不存在的文件 → 失败计数，不中断同批', () async {
    final good = await makeFile('good.txt', 'g'.codeUnits);
    final r = await intake.intakePaths(['${tmp.path}/nope.txt', good]);
    expect(r.summary.failed, 1);
    expect(r.summary.added, 1);
  });
}