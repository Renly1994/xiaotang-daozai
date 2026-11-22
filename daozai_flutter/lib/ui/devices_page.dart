//! 设备页 —— 配对（T1 手动 IP+短码）/设备列表/移除/下载位置（D5）/同步开关。
//! 面板内滚动；SAS 比对对话框走 withDialogRoom 扩窗防裁切。
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import '../core/dialog_room.dart';
import '../core/theme.dart';
import '../services/ports.dart';
import '../state/sync_state.dart';
import 'pages.dart' show PageTitle;

class DevicesPage extends StatefulWidget {
  const DevicesPage({super.key});
  @override
  State<DevicesPage> createState() => _DevicesPageState();
}

class _DevicesPageState extends State<DevicesPage> {
  String? _confirmRemoveFp;

  Future<void> _startPairing(BuildContext context, SyncState s) async {
    final engine = s.engine;
    if (engine == null) return;
    s.setInteractionLockForPairing(true);
    final session = engine.startPairing();
    s.setActivePairing(session);
    if (!context.mounted) return;
    await withDialogRoom(() => showDialog<void>(
          context: context,
          barrierDismissible: false,
          builder: (ctx) => _PairHostDialog(session: session),
        ));
    s.setInteractionLockForPairing(false);
  }

  Future<void> _joinPairing(BuildContext context, SyncState s) async {
    final hostCtrl = TextEditingController();
    final tokenCtrl = TextEditingController();
    s.setInteractionLockForPairing(true);
    final ok = await withDialogRoom(() => showDialog<bool>(
          context: context,
          builder: (ctx) => AlertDialog(
            backgroundColor: DaozaiColors.card,
            title: const Text('加入配对', style: TextStyle(color: DaozaiColors.white, fontSize: 16)),
            content: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                const Text('输入对方设备页显示的地址与配对码', style: TextStyle(color: DaozaiColors.dim, fontSize: 12)),
                const SizedBox(height: 10),
                TextField(
                  controller: hostCtrl,
                  style: const TextStyle(color: DaozaiColors.white, fontSize: 13),
                  decoration: const InputDecoration(hintText: 'IP:端口（如 192.168.1.5:47821）', hintStyle: TextStyle(color: DaozaiColors.dim, fontSize: 12)),
                ),
                TextField(
                  controller: tokenCtrl,
                  style: const TextStyle(color: DaozaiColors.white, fontSize: 13),
                  decoration: const InputDecoration(hintText: '配对码', hintStyle: TextStyle(color: DaozaiColors.dim, fontSize: 12)),
                ),
              ],
            ),
            actions: [
              TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('取消', style: TextStyle(color: DaozaiColors.dim))),
              TextButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('连接', style: TextStyle(color: DaozaiColors.cyan))),
            ],
          ),
        ));
    if (ok == true && hostCtrl.text.contains(':')) {
      final parts = hostCtrl.text.split(':');
      final port = int.tryParse(parts.last);
      if (port != null) {
        await s.engine?.joinPairing(parts.first, port, tokenCtrl.text.trim());
        if (context.mounted && s.activePairing != null) {
          await withDialogRoom(() => showDialog<void>(
                context: context,
                barrierDismissible: false,
                builder: (ctx) => _PairJoinDialog(session: s.activePairing!),
              ));
        }
      } else {
        s.onNotice('端口格式错误');
      }
    }
    hostCtrl.dispose();
    tokenCtrl.dispose();
    s.setInteractionLockForPairing(false);
  }

  Future<void> _changeDownloadDir(BuildContext context, SyncState s) async {
    final engine = s.engine;
    if (engine == null) return;
    try {
      final picked = await engine.download.pickDirectory();
      if (picked == null) return;
      await engine.download.setBaseDir(picked);
      s.setDownloadDir(await engine.download.baseDir());
      s.onNotice('下载位置已更新');
    } catch (e) {
      s.onNotice('$e');
    }
  }

  @override
  Widget build(BuildContext context) {
    final s = context.watch<SyncState>();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const PageTitle('设备', '与同步', DaozaiColors.softGreen),
        if (s.notice != null) ...[
          const SizedBox(height: 4),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
            decoration: BoxDecoration(color: DaozaiColors.red.withOpacity(0.15), borderRadius: BorderRadius.circular(6)),
            child: Row(children: [
              const Icon(Icons.error_outline, size: 13, color: DaozaiColors.red),
              const SizedBox(width: 5),
              Expanded(child: Text(s.notice!, maxLines: 2, overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: DaozaiSize.small, color: DaozaiColors.red))),
            ]),
          ),
        ],
        const SizedBox(height: 6),
        Row(
          children: [
            _btn(Icons.add_link, '发起配对', () => _startPairing(context, s)),
            const SizedBox(width: 6),
            _btn(Icons.login, '加入配对', () => _joinPairing(context, s)),
            const Spacer(),
            Row(mainAxisSize: MainAxisSize.min, children: [
              const Text('同步', style: TextStyle(fontSize: DaozaiSize.small, color: DaozaiColors.dim)),
              SizedBox(
                height: 20,
                child: Switch(
                  value: s.syncEnabled,
                  activeColor: DaozaiColors.green,
                  onChanged: (v) => s.setSyncEnabled(v),
                ),
              ),
            ]),
          ],
        ),
        const SizedBox(height: 6),
        Expanded(
          child: ListView(
            padding: EdgeInsets.zero,
            children: [
              ...s.peers.values.map((p) => _peerRow(s, p)),
              if (s.peers.isEmpty)
                const Padding(
                  padding: EdgeInsets.symmetric(vertical: 8),
                  child: Text('暂无已配对设备', style: TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.dim)),
                ),
              const SizedBox(height: 8),
              // 下载位置（D5）：当前路径 + 更改 + 恢复默认。
              Row(
                children: [
                  const Icon(Icons.folder_outlined, size: 13, color: DaozaiColors.dim),
                  const SizedBox(width: 5),
                  Expanded(
                    child: Text(
                      s.downloadDir.isEmpty ? '下载位置：默认' : '下载：${s.downloadDir}',
                      maxLines: 1, overflow: TextOverflow.ellipsis,
                      style: const TextStyle(fontSize: DaozaiSize.tiny, color: DaozaiColors.dim),
                    ),
                  ),
                  GestureDetector(
                    onTap: () => _changeDownloadDir(context, s),
                    child: const Text('更改', style: TextStyle(fontSize: DaozaiSize.tiny, color: DaozaiColors.cyan)),
                  ),
                  const SizedBox(width: 8),
                  GestureDetector(
                    onTap: () async {
                      await s.engine?.download.resetToDefault();
                      s.setDownloadDir(await s.engine?.download.baseDir() ?? '');
                    },
                    child: const Text('恢复默认', style: TextStyle(fontSize: DaozaiSize.tiny, color: DaozaiColors.dim)),
                  ),
                ],
              ),
            ],
          ),
        ),
      ],
    );
  }

  Widget _peerRow(SyncState s, PeerInfo p) {
    final confirming = _confirmRemoveFp == p.fingerprint;
    return SizedBox(
      height: 28,
      child: Row(
        children: [
          Icon(p.online ? Icons.computer : Icons.computer_outlined, size: 14, color: p.online ? DaozaiColors.green : DaozaiColors.dim),
          const SizedBox(width: 6),
          Expanded(
            child: Text(p.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.white)),
          ),
          Text(p.online ? '在线' : '离线', style: TextStyle(fontSize: DaozaiSize.tiny, color: p.online ? DaozaiColors.green : DaozaiColors.dim)),
          const SizedBox(width: 6),
          GestureDetector(
            onTap: () async {
              if (confirming) {
                await s.engine?.keys.removeTrusted(p.fingerprint);
                setState(() {
                  s.peers.remove(p.fingerprint);
                  _confirmRemoveFp = null;
                });
                s.onNotice('已移除 ${p.name}');
              } else {
                setState(() => _confirmRemoveFp = p.fingerprint);
              }
            },
            child: Icon(confirming ? Icons.delete_forever : Icons.close, size: 13, color: confirming ? DaozaiColors.red : DaozaiColors.dim),
          ),
        ],
      ),
    );
  }

  Widget _btn(IconData icon, String label, VoidCallback onTap) {
    return GestureDetector(
      onTap: onTap,
      child: Container(
        height: 24,
        padding: const EdgeInsets.symmetric(horizontal: 8),
        decoration: BoxDecoration(color: DaozaiColors.btn, borderRadius: BorderRadius.circular(7)),
        child: Row(mainAxisSize: MainAxisSize.min, children: [
          Icon(icon, size: 13, color: DaozaiColors.gray),
          const SizedBox(width: 4),
          Text(label, style: const TextStyle(fontSize: DaozaiSize.small, color: DaozaiColors.gray)),
        ]),
      ),
    );
  }
}

/// advertising 端配对对话框：展示本机地址+token，等对端连入后显示 SAS。
class _PairHostDialog extends StatefulWidget {
  final PairingSessionView session;
  const _PairHostDialog({required this.session});
  @override
  State<_PairHostDialog> createState() => _PairHostDialogState();
}

class _PairHostDialogState extends State<_PairHostDialog> {
  @override
  Widget build(BuildContext context) {
    final s = context.watch<SyncState>();
    final session = s.activePairing ?? widget.session;
    final sas = session.sas;
    return AlertDialog(
      backgroundColor: DaozaiColors.card,
      title: const Text('发起配对', style: TextStyle(color: DaozaiColors.white, fontSize: 16)),
      content: SizedBox(
        width: 320,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (sas == null) ...[
              const Text('在另一台设备上点「加入配对」，输入：', style: TextStyle(color: DaozaiColors.dim, fontSize: 12)),
              const SizedBox(height: 8),
              _copyRow(context, '地址', '${s.listenAddress ?? "本机IP"}:${s.listenPort ?? 47821}'),
              const SizedBox(height: 6),
              _copyRow(context, '配对码', session.token),
              const SizedBox(height: 8),
              const Text('120 秒内有效，用一次即作废', style: TextStyle(color: DaozaiColors.dim, fontSize: 11)),
            ] else ...[
              Text('对方：${session.remoteName ?? "…"}', style: const TextStyle(color: DaozaiColors.white, fontSize: 13)),
              const SizedBox(height: 8),
              const Text('请核对两台设备显示的验证码一致：', style: TextStyle(color: DaozaiColors.dim, fontSize: 12)),
              const SizedBox(height: 6),
              Center(child: Text(sas, style: const TextStyle(color: DaozaiColors.cyan, fontSize: 28, fontWeight: FontWeight.w700, letterSpacing: 4))),
            ],
            if (session.isFailed) ...[
              const SizedBox(height: 8),
              Text(session.failReason ?? '配对失败', style: const TextStyle(color: DaozaiColors.red, fontSize: 12)),
            ],
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () {
            if (sas != null && !session.isFailed) {
              s.engine?.confirmPairing(session.remoteFingerprint ?? '', sas);
            } else {
              s.engine?.cancelPairing(session.remoteFingerprint ?? '');
            }
            Navigator.pop(context);
          },
          child: Text(sas == null ? '取消' : '一致，确认配对', style: const TextStyle(color: DaozaiColors.cyan)),
        ),
      ],
    );
  }

  Widget _copyRow(BuildContext context, String label, String value) {
    return Row(
      children: [
        Text('$label：', style: const TextStyle(color: DaozaiColors.dim, fontSize: 12)),
        Expanded(child: SelectableText(value, style: const TextStyle(color: DaozaiColors.white, fontSize: 12))),
        GestureDetector(
          onTap: () => Clipboard.setData(ClipboardData(text: value)),
          child: const Icon(Icons.copy, size: 13, color: DaozaiColors.dim),
        ),
      ],
    );
  }
}

/// join 端配对对话框：显示 SAS 等用户比对确认。
class _PairJoinDialog extends StatelessWidget {
  final PairingSessionView session;
  const _PairJoinDialog({required this.session});
  @override
  Widget build(BuildContext context) {
    final s = context.watch<SyncState>();
    final cur = s.activePairing ?? session;
    return AlertDialog(
      backgroundColor: DaozaiColors.card,
      title: const Text('加入配对', style: TextStyle(color: DaozaiColors.white, fontSize: 16)),
      content: SizedBox(
        width: 300,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text('对方：${cur.remoteName ?? "…"}', style: const TextStyle(color: DaozaiColors.white, fontSize: 13)),
            const SizedBox(height: 8),
            const Text('请核对两台设备显示的验证码一致：', style: TextStyle(color: DaozaiColors.dim, fontSize: 12)),
            const SizedBox(height: 6),
            Text(cur.sas ?? '…', style: const TextStyle(color: DaozaiColors.cyan, fontSize: 28, fontWeight: FontWeight.w700, letterSpacing: 4)),
            if (cur.isFailed) ...[
              const SizedBox(height: 8),
              Text(cur.failReason ?? '配对失败', style: const TextStyle(color: DaozaiColors.red, fontSize: 12)),
            ],
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () {
            if (cur.sas != null && !cur.isFailed) {
              s.engine?.confirmPairing(cur.remoteFingerprint ?? '', cur.sas!);
            }
            Navigator.pop(context);
          },
          child: const Text('一致，确认配对', style: TextStyle(color: DaozaiColors.cyan)),
        ),
      ],
    );
  }
}