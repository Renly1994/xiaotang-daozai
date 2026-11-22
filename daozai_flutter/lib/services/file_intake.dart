//! 收纳管线 —— 拖入/选中文件 → SHA-256 → 去重 → 预检 → vault → 索引 → 摘要。
//! 原件不动（复制语义）；失败可见不静默；同批部分失败不中断（R3.2–R3.6）。
import 'dart:async';
import 'dart:io';
import '../core/chunker.dart';
import '../core/shelf_index.dart';
import '../state/sync_state.dart';
import 'ports.dart';
import 'sync_engine.dart';

const int defaultMaxFileBytes = 512 * 1024 * 1024; // 512MB

class IntakeResult {
  final IntakeSummary summary;
  IntakeResult(this.summary);
}

class ShelfIntake {
  final BlobVault vault;
  final SyncEngine engine;
  final SyncState state;
  final int maxFileBytes;
  final Future<int> Function() freeDiskBytes;

  ShelfIntake({
    required this.vault,
    required this.engine,
    required this.state,
    this.maxFileBytes = defaultMaxFileBytes,
    Future<int> Function()? freeDiskBytes,
  }) : freeDiskBytes = freeDiskBytes ?? _defaultFreeDisk;

  static Future<int> _defaultFreeDisk() async {
    // dart:io 无直接 statvfs；用系统命令兜底（桌面端）。
    try {
      if (Platform.isMacOS || Platform.isLinux) {
        final r = await Process.run('df', ['-k', '.']);
        final lines = (r.stdout as String).trim().split('\n');
        if (lines.length >= 2) {
          final parts = lines.last.split(RegExp(r'\s+'));
          if (parts.length >= 4) return int.parse(parts[3]) * 1024;
        }
      } else if (Platform.isWindows) {
        final r = await Process.run('powershell', [
          '-NoProfile', '-Command',
          '(Get-PSDrive -Name \$env:SystemDrive.TrimEnd(\':\')).Free',
        ]);
        return int.tryParse((r.stdout as String).trim()) ?? (1 << 62);
      }
    } catch (_) {}
    return 1 << 62; // 探测失败不阻断收纳（vault 写入失败仍会报错）
  }

  /// 收纳一批绝对路径（T1 单文件；文件夹提示下一版，不静默吞）。
  Future<IntakeResult> intakePaths(List<String> paths) async {
    var added = 0, duplicate = 0, failed = 0, skipped = 0;
    final reasons = <String>[];
    final existing = engine.entries;
    for (final p in paths) {
      try {
        final type = await FileSystemEntity.type(p, followLinks: false);
        if (type == FileSystemEntityType.link) {
          skipped++;
          reasons.add('$p: 符号链接跳过');
          continue;
        }
        if (type == FileSystemEntityType.directory) {
          skipped++;
          reasons.add('$p: 文件夹支持将在下一版提供');
          continue;
        }
        if (type != FileSystemEntityType.file) {
          failed++;
          reasons.add('$p: 文件不存在或无法访问');
          continue;
        }
        final f = File(p);
        final stat = await f.stat();
        if (stat.size > maxFileBytes) {
          failed++;
          reasons.add('${f.uri.pathSegments.last}: 超过 ${maxFileBytes ~/ (1024 * 1024)}MB 上限');
          continue;
        }
        // 流式读原件算哈希（原件只读不动）。
        final sink = Sha256Sink();
        final chunks = <List<int>>[];
        await for (final c in f.openRead()) {
          sink.add(c);
          chunks.add(c);
        }
        final sha = sink.digest();
        final name = f.uri.pathSegments.last;
        if (isDuplicate(existing, sha, name)) {
          duplicate++;
          continue;
        }
        // 磁盘预检：vault+download 双份估算（D5）。
        final free = await freeDiskBytes();
        if (free < stat.size * 2 + 1024 * 1024) {
          failed++;
          reasons.add('$name: 磁盘空间不足');
          continue;
        }
        await vault.put(sha, Stream.fromIterable(chunks));
        await engine.registerIntake(
          sha256: sha,
          name: name,
          relPath: name,
          size: stat.size,
          modified: stat.modified,
        );
        added++;
      } catch (e) {
        failed++;
        reasons.add('$p: $e');
      }
    }
    final summary = IntakeSummary(
      added: added, duplicate: duplicate, failed: failed, skipped: skipped,
      failReasons: reasons,
    );
    state.showIntakeSummary(summary);
    if (failed > 0 || skipped > 0) {
      state.onNotice(summary.message);
    }
    return IntakeResult(summary);
  }
}