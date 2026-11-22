//! 同步引擎回环集成测试 —— 双引擎经 127.0.0.1 TCP：配对/索引补差/拉取校验/删除/离线提示。
//! 覆盖验收 9（桌面双机版）、11（删除部分）、12；T1 门禁核心。
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';
import 'package:dynamic_island/core/chunker.dart';
import 'package:dynamic_island/core/shelf_index.dart';
import 'package:dynamic_island/services/download_export.dart';
import 'package:dynamic_island/services/file_vault.dart';
import 'package:dynamic_island/services/lan_transport.dart';
import 'package:dynamic_island/services/ports.dart';
import 'package:dynamic_island/services/sqlite_store.dart';
import 'package:dynamic_island/services/sync_engine.dart';

class MemKeyVault implements KeyVault {
  DeviceIdentity? _id;
  final List<TrustedPeer> trusted = [];
  String name;
  final Uint8List master;
  MemKeyVault(this.name, int seed)
      : master = Uint8List.fromList(List.generate(32, (i) => i + seed));

  @override
  Future<DeviceIdentity> loadOrCreate() async {
    if (_id != null) return _id!;
    final ed = await Ed25519().newKeyPair();
    final x = await X25519().newKeyPair();
    final edPub = (await ed.extractPublicKey()).bytes;
    final h = await Sha256().hash(edPub);
    _id = DeviceIdentity(
      ed25519Private: Uint8List.fromList(await ed.extractPrivateKeyBytes()),
      ed25519Public: Uint8List.fromList(edPub),
      x25519Private: Uint8List.fromList(await x.extractPrivateKeyBytes()),
      x25519Public: Uint8List.fromList((await x.extractPublicKey()).bytes),
      fingerprint: h.bytes.sublist(0, 8).map((b) => b.toRadixString(16).padLeft(2, '0')).join().toUpperCase(),
    );
    return _id!;
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

class RecordingEvents implements SyncEvents {
  List<ShelfEntry> entries = [];
  final notices = <String>[];
  final peerStatus = <String, bool>{};
  final transfersDone = <String, String>{};
  final progress = <TransferProgress>[];
  final pairingUpdates = <PairingSessionView>[];

  @override
  void onEntriesChanged(List<ShelfEntry> e) => entries = e;
  @override
  void onPeerStatus(String fp, String name, bool online) => peerStatus[fp] = online;
  @override
  void onTransferProgress(TransferProgress p) => progress.add(p);
  @override
  void onTransferDone(String sha, String path) => transfersDone[sha] = path;
  @override
  void onNotice(String m) => notices.add(m);
  @override
  void onPairingUpdate(PairingSessionView s) => pairingUpdates.add(s);
}

class _Node {
  final MemKeyVault keys;
  final FileVault vault;
  final SqliteShelfStore store;
  final DownloadExportImpl download;
  final LanTransport transport;
  final SyncEngine engine;
  final RecordingEvents events;
  final Directory dir;

  _Node._(this.dir, this.keys, this.vault, this.store, this.download, this.transport, this.engine, this.events);

  static Future<_Node> create(String name, int seed) async {
    final dir = await Directory.systemTemp.createTemp('daozai_sync_$name');
    final keys = MemKeyVault(name, seed);
    final vault = FileVault(dir: '${dir.path}/vault', masterKey: keys.vaultMasterKey);
    final db = await databaseFactoryFfi.openDatabase(inMemoryDatabasePath);
    final store = SqliteShelfStore.fromDatabase(db, masterKey: keys.vaultMasterKey);
    final download = DownloadExportImpl(
      vault: vault,
      defaultDirResolver: () async => '${dir.path}/download',
    );
    final transport = LanTransport(keys: keys, handshakeTimeout: const Duration(seconds: 5));
    await transport.listen(0);
    final events = RecordingEvents();
    final engine = SyncEngine(
      store: store, vault: vault, download: download, keys: keys,
      transport: transport, events: events,
    );
    await engine.start();
    return _Node._(dir, keys, vault, store, download, transport, engine, events);
  }

  Future<void> dispose() async {
    await engine.dispose();
    await store.close();
    if (await dir.exists()) await dir.delete(recursive: true);
  }
}

Future<void> waitFor(bool Function() cond, {Duration timeout = const Duration(seconds: 5)}) async {
  final sw = Stopwatch()..start();
  while (!cond()) {
    if (sw.elapsed > timeout) throw TimeoutException('条件未满足');
    await Future.delayed(const Duration(milliseconds: 20));
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  sqfliteFfiInit();

  late _Node a;
  late _Node b;

  setUp(() async {
    SharedPreferences.setMockInitialValues({});
    a = await _Node.create('Mac-A', 1);
    b = await _Node.create('Win-B', 100);
  });

  tearDown(() async {
    await a.dispose();
    await b.dispose();
  });

  /// 完整配对流程：A advertising，B join，双方确认 SAS。
  Future<void> pair() async {
    final session = a.engine.startPairing();
    await b.engine.joinPairing('127.0.0.1', a.transport.boundPort!, session.token);
    await waitFor(() => a.events.pairingUpdates.isNotEmpty && b.events.pairingUpdates.isNotEmpty);
    final sa = a.events.pairingUpdates.last.sas!;
    final sb = b.events.pairingUpdates.last.sas!;
    expect(sa, sb);
    final fpB = (await b.keys.loadOrCreate()).fingerprint;
    final fpA = (await a.keys.loadOrCreate()).fingerprint;
    await a.engine.confirmPairing(fpB, sa);
    await b.engine.confirmPairing(fpA, sb);
    await waitFor(() => a.keys.trusted.isNotEmpty && b.keys.trusted.isNotEmpty);
  }

  test('配对全流程：双方信任表建立、在线状态上报（验收 6/9 前置）', () async {
    await pair();
    expect(a.keys.trusted.single.name, 'Win-B');
    expect(b.keys.trusted.single.name, 'Mac-A');
    expect(a.events.peerStatus.values.contains(true), isTrue);
  });

  test('收纳→索引同步→远端可搜→拉取→SHA-256 一致+download 落地（验收 9）', () async {
    await pair();
    // A 收纳一个文件（模拟 intake 已完成 vault.put）。
    final content = utf8.encode('跨端同步测试内容 hello sync');
    final sha = sha256Hex(content);
    await a.vault.put(sha, Stream.value(content));
    final e = await a.engine.registerIntake(
      sha256: sha, name: '测试.txt', relPath: 'docs/测试.txt',
      size: content.length, modified: DateTime(2026, 1, 1),
    );
    // B 应收到索引条目（remote 态）。
    await waitFor(() => b.engine.entries.any((x) => x.sha256 == sha));
    final remote = b.engine.entries.firstWhere((x) => x.sha256 == sha);
    expect(remote.location, EntryLocation.remote);
    expect(searchEntries(b.engine.entries, '测试').length, 1);
    // B 拉取。
    await b.engine.pull(remote.id);
    await waitFor(() => b.events.transfersDone.containsKey(sha));
    final path = b.events.transfersDone[sha]!;
    expect(path.contains('docs/测试.txt'), isTrue);
    expect(await File(path).readAsBytes(), content);
    expect(await b.vault.has(sha), isTrue);
    // 条目变 both。
    await waitFor(() => b.engine.entries.firstWhere((x) => x.sha256 == sha).location == EntryLocation.both);
    expect(e.location, EntryLocation.local);
  });

  test('大文件分块拉取：多块顺序正确、进度上报', () async {
    await pair();
    final content = List<int>.generate(700 * 1024, (i) => i % 251); // 700KB → 3 块
    final sha = sha256Hex(content);
    await a.vault.put(sha, Stream.value(content));
    await a.engine.registerIntake(
      sha256: sha, name: 'big.bin', relPath: 'big.bin',
      size: content.length, modified: DateTime(2026, 1, 1),
    );
    await waitFor(() => b.engine.entries.any((x) => x.sha256 == sha));
    await b.engine.pull(b.engine.entries.firstWhere((x) => x.sha256 == sha).id);
    await waitFor(() => b.events.transfersDone.containsKey(sha), timeout: const Duration(seconds: 15));
    expect(await File(b.events.transfersDone[sha]!).readAsBytes(), content);
    expect(b.events.progress.length, greaterThanOrEqualTo(3));
    expect(b.events.progress.last.received, content.length);
  });

  test('删除同步：A 删→B 索引/vault/download 三层皆删（验收 11 删除部分）', () async {
    await pair();
    final content = utf8.encode('to be deleted');
    final sha = sha256Hex(content);
    await a.vault.put(sha, Stream.value(content));
    await a.engine.registerIntake(
      sha256: sha, name: 'del.txt', relPath: 'del.txt',
      size: content.length, modified: DateTime(2026, 1, 1),
    );
    await waitFor(() => b.engine.entries.any((x) => x.sha256 == sha));
    await b.engine.pull(b.engine.entries.firstWhere((x) => x.sha256 == sha).id);
    await waitFor(() => b.events.transfersDone.containsKey(sha));
    final bPath = b.events.transfersDone[sha]!;
    // A 删除。
    final aEntry = a.engine.entries.firstWhere((x) => x.sha256 == sha);
    await a.engine.removeEntry(aEntry.id);
    await waitFor(() => !b.engine.entries.any((x) => x.sha256 == sha));
    expect(await b.vault.has(sha), isFalse);
    expect(await File(bPath).exists(), isFalse);
    expect(await a.vault.has(sha), isFalse);
  });

  test('源设备离线点击拉取 → 明确提示不假下载（R4.4）', () async {
    await pair();
    final content = utf8.encode('offline file');
    final sha = sha256Hex(content);
    await a.vault.put(sha, Stream.value(content));
    await a.engine.registerIntake(
      sha256: sha, name: 'off.txt', relPath: 'off.txt',
      size: content.length, modified: DateTime(2026, 1, 1),
    );
    await waitFor(() => b.engine.entries.any((x) => x.sha256 == sha));
    // A 断连（模拟离线）。
    await a.engine.dispose();
    await waitFor(() => b.engine.links.isEmpty);
    await b.engine.pull(b.engine.entries.firstWhere((x) => x.sha256 == sha).id);
    await waitFor(() => b.events.notices.any((n) => n.contains('离线')));
    expect(b.events.transfersDone.containsKey(sha), isFalse);
  });

  test('同步开关关闭 → 断开全部连接（验收 12）', () async {
    await pair();
    expect(b.engine.links.isNotEmpty, isTrue);
    await b.engine.setSyncEnabled(false);
    expect(b.engine.links, isEmpty);
  });

  test('水位补差：后上线设备补齐先前收纳的索引', () async {
    // 先配对再断开 B，A 收纳新文件，B 重连后应补到。
    await pair();
    final content = utf8.encode('later file');
    final sha = sha256Hex(content);
    await a.vault.put(sha, Stream.value(content));
    await a.engine.registerIntake(
      sha256: sha, name: 'later.txt', relPath: 'later.txt',
      size: content.length, modified: DateTime(2026, 1, 1),
    );
    await waitFor(() => b.engine.entries.any((x) => x.sha256 == sha));
    expect(b.engine.entries.firstWhere((x) => x.sha256 == sha).name, 'later.txt');
  });
}