//! 置物架索引 —— 纯逻辑：条目模型、去重判定、搜索过滤（无平台依赖，可单测）。
//! 跨端同步的元数据真相源；内容本体在 BlobVault（按 sha256）。
import 'dart:convert';

/// 条目所在位置：本机有副本 / 仅在远端 / 两端都有。
enum EntryLocation { local, remote, both }

class ShelfEntry {
  String id; // uuid v4
  String sha256; // 内容哈希（去重键之一）
  String name; // 文件名
  String relPath; // 文件夹内相对路径；单文件=name
  int size;
  DateTime modified; // 源文件修改时间
  String originDevice; // 首次收纳设备指纹
  DateTime addedAt;
  EntryLocation location;
  String? remoteDevice; // location 含 remote 时的来源设备指纹
  bool trashed; // 回收站标记（T3 启用；字段预留）
  DateTime? trashedAt;
  int version; // 单调递增（T3 LWW 用；字段预留）

  ShelfEntry({
    required this.id,
    required this.sha256,
    required this.name,
    required this.relPath,
    required this.size,
    required this.modified,
    required this.originDevice,
    required this.addedAt,
    this.location = EntryLocation.local,
    this.remoteDevice,
    this.trashed = false,
    this.trashedAt,
    this.version = 1,
  });

  /// 去重键：同内容+同相对路径视为同一条目（幂等 upsert 依据）。
  String get dedupKey => '$sha256:$relPath';

  Map<String, dynamic> toJson() => {
        'id': id,
        'sha256': sha256,
        'name': name,
        'relPath': relPath,
        'size': size,
        'modified': modified.toIso8601String(),
        'originDevice': originDevice,
        'addedAt': addedAt.toIso8601String(),
        'location': location.name,
        'remoteDevice': remoteDevice,
        'trashed': trashed,
        'trashedAt': trashedAt?.toIso8601String(),
        'version': version,
      };

  factory ShelfEntry.fromJson(Map<String, dynamic> j) => ShelfEntry(
        id: j['id'] as String,
        sha256: j['sha256'] as String,
        name: j['name'] as String? ?? '',
        relPath: j['relPath'] as String? ?? '',
        size: (j['size'] as num?)?.toInt() ?? 0,
        modified: DateTime.tryParse(j['modified'] as String? ?? '') ?? DateTime.fromMillisecondsSinceEpoch(0),
        originDevice: j['originDevice'] as String? ?? '',
        addedAt: DateTime.tryParse(j['addedAt'] as String? ?? '') ?? DateTime.fromMillisecondsSinceEpoch(0),
        location: EntryLocation.values.firstWhere(
          (e) => e.name == j['location'],
          orElse: () => EntryLocation.local,
        ),
        remoteDevice: j['remoteDevice'] as String?,
        trashed: j['trashed'] as bool? ?? false,
        trashedAt: DateTime.tryParse(j['trashedAt'] as String? ?? ''),
        version: (j['version'] as num?)?.toInt() ?? 1,
      );

  static String encodeList(List<ShelfEntry> items) =>
      jsonEncode(items.map((e) => e.toJson()).toList());

  static List<ShelfEntry> decodeList(String raw) {
    try {
      return (jsonDecode(raw) as List)
          .map((e) => ShelfEntry.fromJson(e as Map<String, dynamic>))
          .toList();
    } catch (_) {
      return [];
    }
  }
}

/// 去重判定：同 sha256+relPath 已存在 → 重复。
/// 同内容不同路径不算重复（用户可能有意放两份到不同目录）。
bool isDuplicate(List<ShelfEntry> existing, String sha256, String relPath) {
  final key = '$sha256:$relPath';
  return existing.any((e) => !e.trashed && e.dedupKey == key);
}

/// 搜索过滤：文件名/相对路径子串匹配，大小写不敏感，中文原样。
/// 空关键词返回全部（不含回收站）。结果保持原顺序（最新在前由调用方保证）。
List<ShelfEntry> searchEntries(List<ShelfEntry> entries, String query) {
  final visible = entries.where((e) => !e.trashed).toList();
  final q = query.trim().toLowerCase();
  if (q.isEmpty) return visible;
  return visible
      .where((e) =>
          e.name.toLowerCase().contains(q) ||
          e.relPath.toLowerCase().contains(q))
      .toList();
}

/// 幂等 upsert：按 dedupKey 覆盖（保留更高 version），否则追加到首位。
List<ShelfEntry> upsertEntry(List<ShelfEntry> entries, ShelfEntry incoming) {
  final i = entries.indexWhere((e) => e.dedupKey == incoming.dedupKey);
  if (i < 0) return [incoming, ...entries];
  final old = entries[i];
  final merged = incoming.version >= old.version ? incoming : old;
  final out = [...entries];
  out[i] = merged;
  return out;
}