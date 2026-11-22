//! 存储层回归测试 —— file_vault / sqlite_store / download_export（临时目录+内存库，无平台依赖）。
import 'dart:io';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';
import 'package:dynamic_island/core/shelf_index.dart';
import 'package:dynamic_island/services/file_vault.dart';
import 'package:dynamic_island/services/sqlite_store.dart';
import 'package:dynamic_island/services/download_export.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  sqfliteFfiInit();

  late Directory tmp;
  final keyBytes = Uint8List.fromList(List.generate(32, (i) => i));
  Future<Uint8List> masterKey() async => keyBytes;

  setUp(() async {
    tmp = await Directory.systemTemp.createTemp('daozai_store_test');
    SharedPreferences.setMockInitialValues({});
  });

  tearDown(() async {
    if (await tmp.exists()) await tmp.delete(recursive: true);
  });

  group('FileVault', () {
    test('put→get 内容一致；has/usedBytes/delete 正确', () async {
      final v = FileVault(dir: '${tmp.path}/vault', masterKey: masterKey);
      final data = List<int>.generate(5000, (i) => i % 256);
      expect(await v.has('h1'), isFalse);
      await v.put('h1', Stream.value(data));
      expect(await v.has('h1'), isTrue);
      final back = await (await v.get('h1')).toList();
      expect(back.expand((e) => e).toList(), data);
      expect(await v.usedBytes(), greaterThan(5000));
      await v.delete('h1');
      expect(await v.has('h1'), isFalse);
      expect(await v.usedBytes(), 0);
    });

    test('落盘非明文（加密生效）；篡改密文拒绝解密', () async {
      final v = FileVault(dir: '${tmp.path}/vault', masterKey: masterKey);
      const secret = 'TOP SECRET CONTENT 12345';
      await v.put('h2', Stream.value(secret.codeUnits));
      final raw = await File('${tmp.path}/vault/h2.blob').readAsBytes();
      expect(String.fromCharCodes(raw).contains('TOP SECRET'), isFalse);
      raw[20] ^= 0xFF; // 篡改密文
      await File('${tmp.path}/vault/h2.blob').writeAsBytes(raw);
      expect(() async => await (await v.get('h2')).toList(), throwsA(anything));
    });

    test('sweepTemp 清扫半文件；.blob 不受影响', () async {
      final v = FileVault(dir: '${tmp.path}/vault', masterKey: masterKey);
      await v.put('h3', Stream.value([1, 2, 3]));
      await File('${tmp.path}/vault/half.tmp').writeAsString('x');
      await v.sweepTemp();
      expect(await File('${tmp.path}/vault/half.tmp').exists(), isFalse);
      expect(await v.has('h3'), isTrue);
    });

    test('不同主密钥无法解密（密钥隔离）', () async {
      final v1 = FileVault(dir: '${tmp.path}/vault', masterKey: masterKey);
      await v1.put('h4', Stream.value([9, 9, 9]));
      final v2 = FileVault(
        dir: '${tmp.path}/vault',
        masterKey: () async => Uint8List.fromList(List.generate(32, (i) => i + 1)),
      );
      expect(() async => await (await v2.get('h4')).toList(), throwsA(anything));
    });
  });

  group('SqliteShelfStore', () {
    Future<SqliteShelfStore> memStore() async {
      final db = await databaseFactoryFfi.openDatabase(inMemoryDatabasePath);
      return SqliteShelfStore.fromDatabase(db, masterKey: masterKey);
    }

    ShelfEntry e(String sha, String path, {int version = 1, bool trashed = false}) => ShelfEntry(
          id: '$sha:$path',
          sha256: sha,
          name: path.split('/').last,
          relPath: path,
          size: 10,
          modified: DateTime(2026, 1, 1),
          originDevice: 'fpA',
          addedAt: DateTime(2026, 1, 1),
          version: version,
          trashed: trashed,
          trashedAt: trashed ? DateTime(2026, 1, 2) : null,
        );

    test('upsert 幂等：同键不重复；高 version 覆盖低；低不回退', () async {
      final s = await memStore();
      await s.upsert(e('a', 'x.txt'));
      await s.upsert(e('a', 'x.txt', version: 2));
      await s.upsert(e('a', 'x.txt', version: 1));
      final list = await s.loadEntries();
      expect(list.length, 1);
      expect(list.single.version, 2);
      await s.close();
    });

    test('文件名加密落盘：raw 行不含明文名；读取解密还原', () async {
      final db = await databaseFactoryFfi.openDatabase(inMemoryDatabasePath);
      final s = SqliteShelfStore.fromDatabase(db, masterKey: masterKey);
      await s.upsert(e('a', '机密报告.txt'));
      final raw = await db.query('shelf');
      expect(raw.first['nameEnc'], isA<Uint8List>());
      expect(String.fromCharCodes(raw.first['nameEnc'] as Uint8List).contains('机密'), isFalse);
      final back = await s.loadEntries();
      expect(back.single.name, '机密报告.txt');
      await s.close();
    });

    test('remove 与 clearTrash（T3 接口预留可用）', () async {
      final s = await memStore();
      await s.upsert(e('a', 'keep.txt'));
      await s.upsert(e('b', 'gone.txt'));
      await s.upsert(e('c', 'old_trash.txt', trashed: true));
      await s.remove('b:gone.txt');
      await s.clearTrash(DateTime(2026, 1, 3));
      final list = await s.loadEntries();
      expect(list.map((x) => x.relPath), ['keep.txt']);
      await s.close();
    });
  });

  group('DownloadExport', () {
    late FileVault vault;

    setUp(() async {
      vault = FileVault(dir: '${tmp.path}/vault', masterKey: masterKey);
      await vault.put('sha1', Stream.value('hello world'.codeUnits));
    });

    DownloadExportImpl make({Future<String?> Function()? picker}) => DownloadExportImpl(
          vault: vault,
          defaultDirResolver: () async => '${tmp.path}/default_dl',
          directoryPicker: picker,
        );

    test('导出按 relPath 还原结构；内容与 vault 一致', () async {
      final d = make();
      final path = await d.export('sha1', 'docs/sub/a.txt');
      expect(path, '${tmp.path}/default_dl/docs/sub/a.txt');
      expect(await File(path).readAsString(), 'hello world');
    });

    test('目录穿越被安全化拦截', () async {
      final d = make();
      final path = await d.export('sha1', '../../evil.txt');
      expect(path.contains('..'), isFalse);
      expect(await File(path).exists(), isTrue);
    });

    test('手动删后 ensureExported 从 vault 重导（验收 15）', () async {
      final d = make();
      final path = await d.export('sha1', 'a.txt');
      await File(path).delete();
      final again = await d.ensureExported('sha1', 'a.txt');
      expect(await File(again).readAsString(), 'hello world');
    });

    test('自定义目录：切换后新导出去新目录，旧文件不搬家（验收 16）', () async {
      final custom = '${tmp.path}/custom_dl';
      final d = make(picker: () async => custom);
      final old = await d.export('sha1', 'old.txt');
      final picked = await d.pickDirectory();
      await d.setBaseDir(picked!);
      final neu = await d.export('sha1', 'new.txt');
      expect(neu.startsWith(custom), isTrue);
      expect(await File(old).exists(), isTrue); // 不搬家
      expect(await d.baseDir(), custom);
    });

    test('不可写目录 setBaseDir 抛错不落地；恢复默认生效', () async {
      final d = make();
      final badFile = File('${tmp.path}/not_a_dir');
      await badFile.writeAsString('x');
      expect(() => d.setBaseDir(badFile.path), throwsStateError);
      expect(await d.baseDir(), '${tmp.path}/default_dl');
      await d.setBaseDir('${tmp.path}/custom2');
      await d.resetToDefault();
      expect(await d.baseDir(), '${tmp.path}/default_dl');
    });

    test('remove 删除导出文件', () async {
      final d = make();
      final path = await d.export('sha1', 'del.txt');
      await d.remove('del.txt');
      expect(await File(path).exists(), isFalse);
    });
  });
}