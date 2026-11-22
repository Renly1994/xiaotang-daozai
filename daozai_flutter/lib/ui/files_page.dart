//! 文件页 —— 搜索框 + 列表（本地/远端/下载中三态）+ 设备归属角标 + 空状态。
//! 面板 419×140：搜索 26px + 4 行×24px + 圆点；点击远端触发拉取，本地打开。
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../core/shelf_index.dart';
import '../core/theme.dart';
import '../state/sync_state.dart';
import 'pages.dart' show PageTitle;

class FilesPage extends StatefulWidget {
  const FilesPage({super.key});
  @override
  State<FilesPage> createState() => _FilesPageState();
}

class _FilesPageState extends State<FilesPage> {
  final _ctrl = TextEditingController();
  final _focus = FocusNode();
  String _query = '';

  @override
  void dispose() {
    _ctrl.dispose();
    _focus.dispose();
    super.dispose();
  }

  Future<void> _openLocal(BuildContext context, SyncState s, ShelfEntry e) async {
    final engine = s.engine;
    if (engine == null) return;
    try {
      // 缺失重导（验收 15）：download 被手动删 → 从 vault 重新导出。
      final path = await engine.download.ensureExported(e.sha256, e.relPath);
      if (Platform.isMacOS) {
        await Process.run('open', [path]);
      } else if (Platform.isWindows) {
        await Process.run('explorer', [path]);
      }
    } catch (err) {
      s.onNotice('打开失败：$err');
    }
  }

  @override
  Widget build(BuildContext context) {
    final s = context.watch<SyncState>();
    final results = s.search(_query);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const PageTitle('文件', '置物架', DaozaiColors.blue),
        if (s.intakeSummary != null) ...[
          const SizedBox(height: 4),
          _banner(s.intakeSummary!.message, DaozaiColors.cyan, Icons.inventory_2_outlined),
        ],
        if (s.notice != null) ...[
          const SizedBox(height: 4),
          _banner(s.notice!, DaozaiColors.red, Icons.error_outline),
        ],
        const SizedBox(height: 6),
        SizedBox(
          height: 26,
          child: TextField(
            controller: _ctrl,
            focusNode: _focus,
            style: const TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.white),
            decoration: InputDecoration(
              hintText: '搜索文件名…',
              hintStyle: const TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.dim),
              prefixIcon: const Icon(Icons.search, size: 14, color: DaozaiColors.dim),
              prefixIconConstraints: const BoxConstraints(minWidth: 26),
              filled: true, fillColor: DaozaiColors.track,
              contentPadding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
              border: OutlineInputBorder(borderRadius: BorderRadius.circular(7), borderSide: BorderSide.none),
            ),
            onChanged: (v) => setState(() => _query = v),
          ),
        ),
        const SizedBox(height: 6),
        Expanded(
          child: results.isEmpty
              ? Center(
                  child: Text(
                    s.entries.isEmpty ? '拖文件到岛体，或点 + 添加' : '无匹配文件',
                    style: const TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.dim),
                  ),
                )
              : ListView.builder(
                  padding: EdgeInsets.zero,
                  itemCount: results.length,
                  itemBuilder: (_, i) => _FileRow(
                    entry: results[i],
                    progress: s.transfers[results[i].sha256],
                    peerName: s.peers[results[i].remoteDevice ?? results[i].originDevice]?.name,
                    onTap: () async {
                      final e = results[i];
                      if (e.location == EntryLocation.remote) {
                        await s.engine?.pull(e.id);
                      } else {
                        await _openLocal(context, s, e);
                      }
                    },
                  ),
                ),
        ),
      ],
    );
  }

  Widget _banner(String text, Color color, IconData icon) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
      decoration: BoxDecoration(
        color: color.withOpacity(0.15),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Row(
        children: [
          Icon(icon, size: 13, color: color),
          const SizedBox(width: 5),
          Expanded(
            child: Text(text,
                maxLines: 1, overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: DaozaiSize.small, color: color)),
          ),
        ],
      ),
    );
  }
}

class _FileRow extends StatelessWidget {
  final ShelfEntry entry;
  final dynamic progress;
  final String? peerName;
  final VoidCallback onTap;
  const _FileRow({required this.entry, required this.progress, required this.peerName, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final downloading = progress != null;
    final remote = entry.location == EntryLocation.remote;
    final icon = downloading
        ? Icons.downloading
        : remote
            ? Icons.cloud_outlined
            : Icons.insert_drive_file_outlined;
    final color = downloading
        ? DaozaiColors.yellow
        : remote
            ? DaozaiColors.dim
            : DaozaiColors.cyan;
    return GestureDetector(
      onTap: onTap,
      child: SizedBox(
        height: 24,
        child: Row(
          children: [
            Icon(icon, size: 14, color: color),
            const SizedBox(width: 6),
            Expanded(
              child: Text(entry.name,
                  maxLines: 1, overflow: TextOverflow.ellipsis,
                  style: const TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.white)),
            ),
            if (downloading)
              Text('${(progress.fraction * 100).toInt()}%',
                  style: const TextStyle(fontSize: DaozaiSize.tiny, color: DaozaiColors.yellow))
            else if (remote)
              Flexible(
                child: Text(peerName ?? '远端',
                    maxLines: 1, overflow: TextOverflow.ellipsis,
                    style: const TextStyle(fontSize: DaozaiSize.tiny, color: DaozaiColors.dim)),
              ),
          ],
        ),
      ),
    );
  }
}