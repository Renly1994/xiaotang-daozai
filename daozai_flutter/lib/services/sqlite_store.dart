//! 索引库 —— sqflite 实现 ShelfStore。
//! 文件名/relPath 属敏感信息：用 vault 主密钥 AES-GCM 加密后存 BLOB；
//! sha256/size/时间戳明文（无隐私价值，便于排序统计）。搜索时内存解密过滤。
import 'dart:convert';
import 'dart:io' show Platform;
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';
import 'package:sqflite/sqflite.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';
import '../core/shelf_index.dart';
import 'ports.dart';

class SqliteShelfStore implements ShelfStore {
  final String dbPath;
  final Future<Uint8List> Function() masterKey;
  final _algo = AesGcm.with256bits();
  Database? _db;
  SecretKey? _key;

  SqliteShelfStore({required this.dbPath, required this.masterKey});

  /// 测试注入：内存库（ffi）直接传入，跳过文件路径。
  SqliteShelfStore.fromDatabase(Database db, {required this.masterKey}) : dbPath = '' {
    _db = db;
    _ensureSchema(db);
  }

  static const _schema = '''
    CREATE TABLE IF NOT EXISTS shelf(
      id TEXT PRIMARY KEY,
      dedupKey TEXT NOT NULL UNIQUE,
      sha256 TEXT NOT NULL,
      nameEnc BLOB NOT NULL,
      relPathEnc BLOB NOT NULL,
      size INTEGER NOT NULL,
      modified TEXT NOT NULL,
      originDevice TEXT NOT NULL,
      addedAt TEXT NOT NULL,
      location TEXT NOT NULL,
      remoteDevice TEXT,
      trashed INTEGER NOT NULL DEFAULT 0,
      trashedAt TEXT,
      version INTEGER NOT NULL DEFAULT 1
    )''';

  Future<void> _ensureSchema(Database db) => db.execute(_schema);

  static void initFfi() {
    if (Platform.isWindows || Platform.isLinux) {
      sqfliteFfiInit();
      databaseFactory = databaseFactoryFfi;
    }
  }

  Future<SecretKey> _secret() async {
    _key ??= SecretKey(await masterKey());
    return _key!;
  }

  Future<Database> _open() async {
    if (_db != null) return _db!;
    _db = await openDatabase(
      dbPath,
      version: 1,
      onCreate: (db, v) => db.execute(_schema),
    );
    return _db!;
  }

  Future<Uint8List> _enc(String plain) async {
    final key = await _secret();
    final nonce = _algo.newNonce();
    final box = await _algo.encrypt(utf8.encode(plain), secretKey: key, nonce: nonce);
    final out = BytesBuilder(copy: false);
    out.add(nonce);
    out.add(box.cipherText);
    out.add(box.mac.bytes);
    return out.toBytes();
  }

  Future<String> _dec(Uint8List enc) async {
    final key = await _secret();
    final box = SecretBox(
      enc.sublist(12, enc.length - 16),
      nonce: enc.sublist(0, 12),
      mac: Mac(enc.sublist(enc.length - 16)),
    );
    return utf8.decode(await _algo.decrypt(box, secretKey: key));
  }

  @override
  Future<List<ShelfEntry>> loadEntries() async {
    final db = await _open();
    final rows = await db.query('shelf', orderBy: 'addedAt DESC');
    final out = <ShelfEntry>[];
    for (final r in rows) {
      out.add(ShelfEntry(
        id: r['id'] as String,
        sha256: r['sha256'] as String,
        name: await _dec(r['nameEnc'] as Uint8List),
        relPath: await _dec(r['relPathEnc'] as Uint8List),
        size: r['size'] as int,
        modified: DateTime.parse(r['modified'] as String),
        originDevice: r['originDevice'] as String,
        addedAt: DateTime.parse(r['addedAt'] as String),
        location: EntryLocation.values.firstWhere((e) => e.name == r['location'], orElse: () => EntryLocation.local),
        remoteDevice: r['remoteDevice'] as String?,
        trashed: (r['trashed'] as int) != 0,
        trashedAt: r['trashedAt'] == null ? null : DateTime.parse(r['trashedAt'] as String),
        version: r['version'] as int,
      ));
    }
    return out;
  }

  @override
  Future<void> upsert(ShelfEntry e) async {
    final db = await _open();
    // 幂等：同 dedupKey 保留更高 version（T1 纯覆盖语义，T3 加冲突判定）。
    final old = await db.query('shelf', where: 'dedupKey = ?', whereArgs: [e.dedupKey], limit: 1);
    if (old.isNotEmpty && (old.first['version'] as int) > e.version) return;
    await db.insert(
      'shelf',
      {
        'id': e.id,
        'dedupKey': e.dedupKey,
        'sha256': e.sha256,
        'nameEnc': await _enc(e.name),
        'relPathEnc': await _enc(e.relPath),
        'size': e.size,
        'modified': e.modified.toIso8601String(),
        'originDevice': e.originDevice,
        'addedAt': e.addedAt.toIso8601String(),
        'location': e.location.name,
        'remoteDevice': e.remoteDevice,
        'trashed': e.trashed ? 1 : 0,
        'trashedAt': e.trashedAt?.toIso8601String(),
        'version': e.version,
      },
      conflictAlgorithm: ConflictAlgorithm.replace,
    );
  }

  @override
  Future<void> remove(String entryId) async {
    final db = await _open();
    await db.delete('shelf', where: 'id = ?', whereArgs: [entryId]);
  }

  @override
  Future<void> clearTrash(DateTime before) async {
    final db = await _open();
    await db.delete('shelf',
        where: 'trashed = 1 AND trashedAt IS NOT NULL AND trashedAt < ?',
        whereArgs: [before.toIso8601String()]);
  }

  Future<void> close() async {
    await _db?.close();
    _db = null;
  }
}