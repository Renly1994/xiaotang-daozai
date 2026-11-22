//! 明文导出层（D5）—— download/ 目录管理：按 relPath 还原结构、自定义目录、失效回落。
//! vault 是同步真相源；本层只负责把校验通过的内容解密导出给用户看。
import 'dart:io';
import 'package:path_provider/path_provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import '../core/chunker.dart';
import 'ports.dart';

class DownloadExportImpl implements DownloadExport {
  final BlobVault vault;
  final Future<String> Function() defaultDirResolver;
  final Future<String?> Function()? directoryPicker; // 系统选择器注入（测试 fake）
  static const _kCustomDir = 'daozai.download.dir';
  String? _customDir;
  bool _loaded = false;

  DownloadExportImpl({
    required this.vault,
    Future<String> Function()? defaultDirResolver,
    this.directoryPicker,
  }) : defaultDirResolver = defaultDirResolver ?? platformDefaultDir;

  /// 平台默认目录（design §2.3 D5 表）。
  static Future<String> platformDefaultDir() async {
    if (Platform.isMacOS) {
      final support = await getApplicationSupportDirectory();
      return '${support.path}/download';
    }
    if (Platform.isWindows) {
      // 安装目录可写 → 用之；否则回落 LOCALAPPDATA。
      final exeDir = File(Platform.resolvedExecutable).parent.path;
      final candidate = '$exeDir\\download';
      if (await _writableAt(candidate)) return candidate;
      final local = Platform.environment['LOCALAPPDATA'];
      return '$local\\DynamicIsland\\download';
    }
    if (Platform.isAndroid) {
      final ext = await getExternalStorageDirectory();
      return '${ext!.path}/download';
    }
    final support = await getApplicationSupportDirectory();
    return '${support.path}/download';
  }

  static Future<bool> _writableAt(String dir) async {
    try {
      final d = Directory(dir);
      await d.create(recursive: true);
      final probe = File('${d.path}/.daozai_probe');
      await probe.writeAsString('x');
      await probe.delete();
      return true;
    } catch (_) {
      return false;
    }
  }

  Future<void> _load() async {
    if (_loaded) return;
    final prefs = await SharedPreferences.getInstance();
    _customDir = prefs.getString(_kCustomDir);
    _loaded = true;
  }

  @override
  Future<String> baseDir() async {
    await _load();
    if (_customDir != null && await _writableAt(_customDir!)) return _customDir!;
    return defaultDirResolver();
  }

  @override
  Future<bool> writable() async => _writableAt(await baseDir());

  @override
  Future<String> export(String sha256, String relPath) async {
    final safe = sanitizeRelPath(relPath);
    if (safe.isEmpty) throw StateError('非法路径: $relPath');
    var dir = await baseDir();
    if (!await _writableAt(dir)) {
      // 自定义目录失效 → 回落默认完成导出，不丢文件（调用方负责红条提示）。
      dir = await defaultDirResolver();
      if (!await _writableAt(dir)) throw StateError('下载目录不可写: $dir');
    }
    final target = File('$dir/$safe');
    await target.parent.create(recursive: true);
    final tmp = File('${target.path}.daozai_tmp');
    final stream = await vault.get(sha256);
    final sink = tmp.openWrite();
    try {
      await stream.forEach(sink.add);
      await sink.flush();
    } finally {
      await sink.close();
    }
    await tmp.rename(target.path); // 原子改名：download/ 里只有完整导出
    return target.path;
  }

  @override
  Future<void> remove(String relPath) async {
    final safe = sanitizeRelPath(relPath);
    if (safe.isEmpty) return;
    final dir = await baseDir();
    final f = File('$dir/$safe');
    if (await f.exists()) await f.delete();
  }

  @override
  Future<String?> pickDirectory() async {
    if (directoryPicker == null) return null;
    return directoryPicker!();
  }

  @override
  Future<void> setBaseDir(String picked) async {
    if (!await _writableAt(picked)) {
      throw StateError('所选目录不可写: $picked');
    }
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kCustomDir, picked);
    _customDir = picked;
    _loaded = true;
  }

  @override
  Future<void> resetToDefault() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.remove(_kCustomDir);
    _customDir = null;
    _loaded = true;
  }

  @override
  Future<String> ensureExported(String sha256, String relPath) async {
    final safe = sanitizeRelPath(relPath);
    final dir = await baseDir();
    final f = File('$dir/$safe');
    if (await f.exists()) return f.path;
    return export(sha256, relPath);
  }
}

/// 桌面系统目录选择器（macOS osascript / Windows PowerShell FolderBrowserDialog）。
/// 返回选中绝对路径；取消返回 null。测试经 directoryPicker 注入 fake。
Future<String?> systemDirectoryPicker() async {
  try {
    if (Platform.isMacOS) {
      final r = await Process.run('osascript', [
        '-e',
        'POSIX path of (choose folder with prompt "选择下载目录")',
      ]);
      if (r.exitCode != 0) return null;
      final p = (r.stdout as String).trim();
      return p.isEmpty ? null : p;
    }
    if (Platform.isWindows) {
      final r = await Process.run('powershell', [
        '-NoProfile', '-Command',
        r'Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; if ($d.ShowDialog() -eq "OK") { $d.SelectedPath }',
      ]);
      if (r.exitCode != 0) return null;
      final p = (r.stdout as String).trim();
      return p.isEmpty ? null : p;
    }
  } catch (_) {}
  return null;
}