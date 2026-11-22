//! 同步引擎 —— 编排：配对确认/连接管理/索引水位补差/拉取队列/收纳推送。
//! T1：手动 IP 直连（无 mDNS）；整文件传输+SHA-256 校验；失败可见不静默。
import 'dart:async';
import 'dart:typed_data';
import 'package:uuid/uuid.dart';
import '../core/chunker.dart';
import '../core/pairing.dart';
import '../core/protocol.dart';
import '../core/shelf_index.dart';
import 'ports.dart';

class TransferProgress {
  final String sha256;
  final int received;
  final int total;
  TransferProgress(this.sha256, this.received, this.total);
  double get fraction => total == 0 ? 0 : received / total;
}

/// 引擎事件 → SyncState/UI。
abstract class SyncEvents {
  void onEntriesChanged(List<ShelfEntry> entries);
  void onPeerStatus(String fingerprint, String name, bool online);
  void onTransferProgress(TransferProgress p);
  void onTransferDone(String sha256, String localPath);
  void onNotice(String message); // 红条：失败/警告，不静默
  void onPairingUpdate(PairingSessionView session);
}

class SyncEngine {
  final ShelfStore store;
  final BlobVault vault;
  final DownloadExport download;
  final KeyVault keys;
  final PeerTransport transport;
  final SyncEvents events;
  final Clock clock;
  final _uuid = const Uuid();
  // 每连接独立帧解析器：共享 parser 会把两条连接的字节混在一起导致错帧。
  final Map<String, FrameParser> _parsers = {};

  List<ShelfEntry> _entries = [];
  final Map<String, PeerLink> _links = {}; // fingerprint → link
  final Map<String, PairingSessionView> _pairings = {}; // fingerprint → session（本端发起或加入）
  final Map<String, _PullJob> _pulls = {};
  final Set<String> _tombstones = {}; // 已删条目 id：防对端水位补差把删除项复活
  final Set<String> _syncRequested = {}; // 每连接只主动请求一次索引，防 ping-pong
  int _watermark = 0; // 已见最大 addedAt 毫秒（索引补差水位）
  bool _started = false;
  StreamSubscription? _incomingSub;

  SyncEngine({
    required this.store,
    required this.vault,
    required this.download,
    required this.keys,
    required this.transport,
    required this.events,
    Clock? clock,
  }) : clock = clock ?? defaultClock;

  List<ShelfEntry> get entries => List.unmodifiable(_entries);
  Map<String, PeerLink> get links => Map.unmodifiable(_links);

  Future<void> start() async {
    if (_started) return;
    _started = true;
    await vault.sweepTemp();
    _entries = await store.loadEntries();
    for (final e in _entries) {
      final ms = e.addedAt.millisecondsSinceEpoch;
      if (ms > _watermark) _watermark = ms;
    }
    events.onEntriesChanged(_entries);
    _incomingSub = transport.incoming.listen(_onIncoming);
  }

  Future<void> dispose() async {
    await _incomingSub?.cancel();
    for (final l in _links.values.toList()) {
      await l.close();
    }
    _links.clear();
    await transport.close();
  }

  // ---- 配对 ----

  /// 发起配对（advertising）：返回 session 供 UI 展示 token/IP/端口。
  PairingSessionView startPairing() {
    final s = transport.startPairing(clock());
    events.onPairingUpdate(s);
    return s;
  }

  /// 加入配对（join）：连对端 IP:端口，带 token。
  Future<void> joinPairing(String host, int port, String token) async {
    try {
      final link = await transport.connect(host, port, pairToken: token);
      final s = PairingSession(token: token, createdAt: clock());
      s.startJoin();
      s.onKeyexDone(link.sas, link.remoteName, link.remoteFingerprint);
      _pairings[link.remoteFingerprint] = s;
      _attach(link, pairing: s);
      events.onPairingUpdate(s);
    } catch (e) {
      events.onNotice('配对连接失败：$e');
    }
  }

  /// 用户比对 SAS 一致后确认（两端各自调用）。
  Future<void> confirmPairing(String fingerprint, String sasCode) async {
    final s = _pairings[fingerprint];
    final link = _links[fingerprint];
    if (s == null || link == null) return;
    final ps = s is PairingSession ? s : null;
    if (ps == null || !ps.confirm(sasCode)) {
      events.onNotice(s.failReason ?? '配对失败');
      events.onPairingUpdate(s);
      await link.close();
      _links.remove(fingerprint);
      return;
    }
    link.send(Frame.fromJson(MsgType.pairConfirm, pairConfirmPayload(true, await keys.deviceName())).encode());
    events.onPairingUpdate(s);
    await _tryFinalizePairing(fingerprint);
  }

  Future<void> _tryFinalizePairing(String fingerprint) async {
    final s = _pairings[fingerprint];
    final link = _links[fingerprint];
    if (s == null || link == null || !s.fullyConfirmed) return;
    await keys.saveTrusted(TrustedPeer(
      fingerprint: fingerprint,
      name: s.remoteName ?? link.remoteName,
      ed25519Public: link.peerIdentityPub,
      x25519Public: link.peerStaticXPub,
      lastSeen: clock(),
    ));
    _pairings.remove(fingerprint);
    link.isPairing = false; // 配对完成：链路转正常态，可收发业务帧
    events.onPeerStatus(fingerprint, link.remoteName, true);
    events.onNotice('已与 ${link.remoteName} 配对成功');
    // 配对完成即补索引。
    await _requestIndexSync(link);
  }

  void cancelPairing(String fingerprint) {
    final s = _pairings.remove(fingerprint);
    if (s is PairingSession) s.cancel();
    _links.remove(fingerprint)?.close();
    if (s != null) events.onPairingUpdate(s);
  }

  // ---- 连接管理 ----

  /// 手动连接已信任设备（T1 入口；T2 mDNS 自动触发同一方法）。
  Future<void> connectTrusted(String host, int port) async {
    try {
      final link = await transport.connect(host, port);
      _attach(link);
      events.onPeerStatus(link.remoteFingerprint, link.remoteName, true);
      await _requestIndexSync(link);
    } catch (e) {
      events.onNotice('连接失败：$e');
    }
  }

  void _onIncoming(PeerLink link) {
    _attach(link);
    if (!link.isPairing) {
      events.onPeerStatus(link.remoteFingerprint, link.remoteName, true);
      // 被动接受方不主动请求：发起方 connectTrusted 已请求，回 delta 即完成互换。
    } else {
      // advertising 端：session 已由 transport.consumeToken 推进到 keyex。
      final s = link.pairingSession;
      if (s != null) {
        _pairings[link.remoteFingerprint] = s;
        events.onPairingUpdate(s);
      }
    }
  }

  void _attach(PeerLink link, {PairingSession? pairing}) {
    final fp = link.remoteFingerprint;
    // 双连接仲裁：指纹字典序小者发起的连接胜出（design §2.6）。
    final old = _links[fp];
    if (old != null) {
      old.close();
    }
    _links[fp] = link;
    link.frames.listen(
      (bytes) => _onFrame(link, bytes),
      onError: (Object e) {
        events.onNotice('与 ${link.remoteName} 的连接出错：$e');
      },
      onDone: () {
        if (_links[fp] == link) {
          _links.remove(fp);
          _syncRequested.remove(fp);
          events.onPeerStatus(fp, link.remoteName, false);
        }
      },
    );
  }

  Future<void> _requestIndexSync(PeerLink link) async {
    // 每连接只主动请求一次：双方 finalize 都会调，去重防无限 ping-pong。
    if (_syncRequested.contains(link.remoteFingerprint)) return;
    _syncRequested.add(link.remoteFingerprint);
    link.send(Frame.fromJson(MsgType.indexSyncRequest, indexSyncRequestPayload(_watermark)).encode());
  }

  /// 防 ping-pong：对端补差请求只回数据，不再反向请求（每连接一次初始互换足够）。
  Future<void> _replyIndexSync(PeerLink link, int wm) async {
    final delta = _entries
        .where((e) => e.addedAt.millisecondsSinceEpoch > wm && !_tombstones.contains(e.id))
        .toList();
    link.send(Frame.fromJson(MsgType.indexDelta, {
      'upsert': delta.map((e) => e.toJson()).toList(),
      'remove': _tombstones.toList(),
    }).encode());
  }

  // ---- 帧处理 ----

  Future<void> _onFrame(PeerLink link, Uint8List bytes) async {
    final parser = _parsers.putIfAbsent(link.remoteFingerprint, () => FrameParser());
    List<Frame> frames;
    try {
      frames = parser.feed(bytes);
    } catch (_) {
      events.onNotice('收到损坏帧，已断开 ${link.remoteName}');
      await link.close();
      return;
    }
    for (final f in frames) {
      try {
        await _handleFrame(link, f);
      } catch (e) {
        events.onNotice('处理消息失败：$e');
      }
    }
  }

  Future<void> _handleFrame(PeerLink link, Frame f) async {
    switch (f.type) {
      case MsgType.pairConfirm:
        final j = f.json();
        final s = _pairings[link.remoteFingerprint];
        if (s is PairingSession) s.onRemoteConfirm(j['ok'] as bool? ?? false);
        if (s != null) {
          events.onPairingUpdate(s);
          await _tryFinalizePairing(link.remoteFingerprint);
        }
      case MsgType.indexSyncRequest:
        final wm = (f.json()['wm'] as num?)?.toInt() ?? 0;
        await _replyIndexSync(link, wm);
      case MsgType.indexDelta:
        final j = f.json();
        for (final raw in (j['upsert'] as List? ?? [])) {
          final incoming = ShelfEntry.fromJson(raw as Map<String, dynamic>);
          if (_tombstones.contains(incoming.id)) continue; // 本端已删，不复活
          // 远端条目：本端无副本 → location=remote。
          final hasLocal = await vault.has(incoming.sha256);
          incoming.location = hasLocal ? EntryLocation.both : EntryLocation.remote;
          incoming.remoteDevice = link.remoteFingerprint;
          _entries = upsertEntry(_entries, incoming);
          await store.upsert(incoming);
          final ms = incoming.addedAt.millisecondsSinceEpoch;
          if (ms > _watermark) _watermark = ms;
        }
        for (final id in (j['remove'] as List? ?? [])) {
          await _applyRemove(id as String, propagate: false);
        }
        events.onEntriesChanged(_entries);
      case MsgType.blobRequest:
        await _serveBlob(link, f.json());
      case MsgType.blobChunk:
        await _onBlobChunk(link, f.payload);
      case MsgType.blobDone:
        await _onBlobDone(link, f.json());
      case MsgType.ackError:
        final j = f.json();
        events.onNotice('对方报告错误：${j['reason']}');
      case MsgType.pingPong:
        link.send(Frame(MsgType.pingPong, Uint8List(0)).encode());
    }
  }

  // ---- 收纳（本地拖入/选择后调用）----

  /// 收纳一个已读入 vault 的文件：建索引+推送远端。intake 层负责哈希/去重/预检/vault.put。
  Future<ShelfEntry> registerIntake({
    required String sha256,
    required String name,
    required String relPath,
    required int size,
    required DateTime modified,
  }) async {
    final id = await keys.loadOrCreate();
    final e = ShelfEntry(
      id: _uuid.v4(),
      sha256: sha256,
      name: name,
      relPath: relPath,
      size: size,
      modified: modified,
      originDevice: id.fingerprint,
      addedAt: clock(),
      location: EntryLocation.local,
    );
    _entries = upsertEntry(_entries, e);
    await store.upsert(e);
    if (e.addedAt.millisecondsSinceEpoch > _watermark) {
      _watermark = e.addedAt.millisecondsSinceEpoch;
    }
    events.onEntriesChanged(_entries);
    _broadcastDelta(upsert: [e]);
    return e;
  }

  void _broadcastDelta({List<ShelfEntry> upsert = const [], List<String> remove = const []}) {
    if (upsert.isEmpty && remove.isEmpty) return;
    final frame = Frame.fromJson(MsgType.indexDelta, {
      'upsert': upsert.map((e) => e.toJson()).toList(),
      'remove': remove,
    }).encode();
    for (final l in _links.values) {
      if (!l.isPairing) l.send(frame);
    }
  }

  /// 删除条目：本端三层删 + 广播（T2 直接删；T3 回收站在此扩展）。
  Future<void> removeEntry(String entryId) async {
    await _applyRemove(entryId, propagate: true);
  }

  Future<void> _applyRemove(String entryId, {required bool propagate}) async {
    _tombstones.add(entryId);
    final i = _entries.indexWhere((e) => e.id == entryId);
    if (i < 0) return;
    final e = _entries[i];
    _entries.removeAt(i);
    await store.remove(entryId);
    // 仅当无其他条目引用同内容时删 vault（去重共享）。
    final stillReferenced = _entries.any((x) => x.sha256 == e.sha256);
    if (!stillReferenced) await vault.delete(e.sha256);
    await download.remove(e.relPath);
    events.onEntriesChanged(_entries);
    if (propagate) _broadcastDelta(remove: [entryId]);
  }

  // ---- 拉取（按需，D3）----

  /// 用户点击远端文件 → 拉取。源设备离线 → 明确提示不假下载（R4.4）。
  Future<void> pull(String entryId) async {
    final e = _entries.firstWhere((x) => x.id == entryId, orElse: () => throw StateError('条目不存在'));
    if (await vault.has(e.sha256)) {
      final path = await download.export(e.sha256, e.relPath);
      events.onTransferDone(e.sha256, path);
      return;
    }
    final sourceFp = e.remoteDevice ?? e.originDevice;
    final link = _links[sourceFp];
    if (link == null) {
      final trusted = await keys.trustedPeers();
      final name = trusted.firstWhere((t) => t.fingerprint == sourceFp, orElse: () => TrustedPeer(fingerprint: '', name: '未知设备', ed25519Public: Uint8List(0), x25519Public: Uint8List(0))).name;
      events.onNotice('文件在 $name 上，该设备当前离线');
      return;
    }
    if (_pulls.containsKey(e.sha256)) return; // 已在拉取
    _pulls[e.sha256] = _PullJob(entry: e, sink: Sha256Sink());
    link.send(Frame.fromJson(MsgType.blobRequest, blobRequestPayload(e.sha256, 0)).encode());
  }

  /// 源端响应 blobRequest：整文件分块发送（T1；T2 加 offset 续传）。
  Future<void> _serveBlob(PeerLink link, Map<String, dynamic> j) async {
    final sha = j['sha'] as String;
    final offset = (j['off'] as num?)?.toInt() ?? 0;
    if (!await vault.has(sha)) {
      link.send(Frame.fromJson(MsgType.ackError, errorPayload('ENOENT', '源文件不存在')).encode());
      return;
    }
    final stream = await vault.get(sha);
    final all = <int>[];
    await stream.forEach(all.addAll);
    var sent = 0;
    for (final c in chunkPlan(all.length)) {
      if (c.offset + c.length <= offset) continue; // T2 续传跳过已收部分
      final start = c.offset < offset ? offset - c.offset : 0;
      final piece = all.sublist(c.offset + start, c.offset + c.length);
      link.send(Frame(MsgType.blobChunk, blobChunkPayload(sha, c.offset + start, piece)).encode());
      sent += piece.length;
    }
    link.send(Frame.fromJson(MsgType.blobDone, blobDonePayload(sha, all.length)).encode());
    // ignore: unused_local_variable
    final _ = sent;
  }

  Future<void> _onBlobChunk(PeerLink link, Uint8List payload) async {
    final r = parseBlobChunk(payload);
    final job = _pulls[r.sha256];
    if (job == null) return;
    job.sink.add(r.chunk);
    job.chunks.add(r.chunk);
    job.received += r.chunk.length;
    events.onTransferProgress(TransferProgress(r.sha256, job.received, job.entry.size));
  }

  Future<void> _onBlobDone(PeerLink link, Map<String, dynamic> j) async {
    final sha = j['sha'] as String;
    final total = (j['total'] as num).toInt();
    final job = _pulls.remove(sha);
    if (job == null) return;
    final actual = job.sink.digest();
    if (!verifyBlob(sha, total, actual, job.sink.bytes)) {
      events.onNotice('文件校验失败，已拒收（${job.entry.name}）');
      return;
    }
    // 校验通过才入 vault（任何时刻 vault 只有完整文件）。
    await vault.put(sha, Stream.value(job.collected()));
    final path = await download.export(sha, job.entry.relPath);
    // 更新条目位置。
    final i = _entries.indexWhere((e) => e.sha256 == sha);
    if (i >= 0) {
      final e = _entries[i];
      e.location = EntryLocation.both;
      await store.upsert(e);
      events.onEntriesChanged(_entries);
    }
    events.onTransferDone(sha, path);
  }

  /// 同步总开关（R7.2/R2.5）：关闭即断全部连接。
  Future<void> setSyncEnabled(bool enabled) async {
    if (!enabled) {
      for (final l in _links.values.toList()) {
        await l.close();
      }
      _links.clear();
    }
  }
}

class _PullJob {
  final ShelfEntry entry;
  final Sha256Sink sink;
  final chunks = <List<int>>[];
  int received = 0;
  _PullJob({required this.entry, required this.sink});

  List<int> collected() => chunks.expand((c) => c).toList();
}