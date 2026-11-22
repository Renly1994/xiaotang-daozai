//! 同步状态 —— ChangeNotifier：设备/传输/收纳摘要/红条/投放态/下载位置。
//! 模式对齐 AppState.reminderNotice：失败可见，成功清除，不静默（R7.1）。
import 'dart:async';
import 'package:flutter/foundation.dart';
import '../core/shelf_index.dart';
import '../services/ports.dart';
import '../services/sync_engine.dart';

class PeerInfo {
  final String fingerprint;
  String name;
  bool online;
  DateTime? lastSeen;
  PeerInfo(this.fingerprint, this.name, {this.online = false, this.lastSeen});
}

class IntakeSummary {
  final int added;
  final int duplicate;
  final int failed;
  final int skipped;
  final List<String> failReasons;
  IntakeSummary({this.added = 0, this.duplicate = 0, this.failed = 0, this.skipped = 0, this.failReasons = const []});

  String get message {
    if (added == 0 && failed == 0 && skipped == 0) {
      return duplicate == 1 ? '该文件已在置物架中' : '这些文件已在置物架中';
    }
    if (added == 0 && duplicate == 0 && skipped == 0) return '文件无法添加';
    final parts = <String>[];
    if (added > 0) parts.add('已添加 $added 个');
    if (duplicate > 0) parts.add('$duplicate 个已存在');
    if (skipped > 0) parts.add('$skipped 个跳过');
    if (failed > 0) parts.add('$failed 个失败');
    return parts.join('，');
  }
}

class SyncState extends ChangeNotifier implements SyncEvents {
  SyncEngine? _engine;
  SyncEngine? get engine => _engine;

  List<ShelfEntry> entries = [];
  final Map<String, PeerInfo> peers = {};
  final Map<String, TransferProgress> transfers = {}; // sha256 → 进行中
  String? notice; // 红条
  IntakeSummary? intakeSummary; // 收纳结果摘要（短暂显示）
  bool dropHot = false; // 投放态
  bool syncEnabled = true;
  String downloadDir = '';
  String? listenAddress; // 本机局域网 IP（设备页展示）
  int? listenPort;
  bool pairingLock = false; // 配对对话框期间岛体不收起
  PairingSessionView? activePairing; // 当前配对会话（UI 展示 SAS/token）
  Timer? _summaryTimer;
  Timer? _noticeTimer;

  void bind(SyncEngine engine) {
    _engine = engine;
  }

  // ---- SyncEvents ----

  @override
  void onEntriesChanged(List<ShelfEntry> e) {
    entries = e;
    notifyListeners();
  }

  @override
  void onPeerStatus(String fingerprint, String name, bool online) {
    final p = peers.putIfAbsent(fingerprint, () => PeerInfo(fingerprint, name));
    p.name = name;
    p.online = online;
    if (!online) p.lastSeen = DateTime.now();
    notifyListeners();
  }

  @override
  void onTransferProgress(TransferProgress p) {
    transfers[p.sha256] = p;
    notifyListeners();
  }

  @override
  void onTransferDone(String sha256, String localPath) {
    transfers.remove(sha256);
    notifyListeners();
  }

  @override
  void onNotice(String message) {
    notice = message;
    _noticeTimer?.cancel();
    _noticeTimer = Timer(const Duration(seconds: 8), () {
      notice = null;
      notifyListeners();
    });
    notifyListeners();
  }

  @override
  void onPairingUpdate(PairingSessionView session) {
    activePairing = session.isFailed || session.fullyConfirmed ? null : session;
    if (session.isFailed && session.failReason != null) {
      onNotice(session.failReason!);
    }
    notifyListeners();
  }

  // ---- UI 操作 ----

  void setDropHot(bool v) {
    if (dropHot == v) return;
    dropHot = v;
    notifyListeners();
  }

  void setInteractionLockForPairing(bool v) {
    if (pairingLock == v) return;
    pairingLock = v;
    notifyListeners();
  }

  void setListenInfo(String? address, int? port) {
    listenAddress = address;
    listenPort = port;
    notifyListeners();
  }

  void showIntakeSummary(IntakeSummary s) {
    intakeSummary = s;
    _summaryTimer?.cancel();
    _summaryTimer = Timer(const Duration(seconds: 6), () {
      intakeSummary = null;
      notifyListeners();
    });
    notifyListeners();
  }

  void clearNotice() {
    notice = null;
    _noticeTimer?.cancel();
    notifyListeners();
  }

  Future<void> setSyncEnabled(bool v) async {
    syncEnabled = v;
    await _engine?.setSyncEnabled(v);
    notifyListeners();
  }

  void setDownloadDir(String dir) {
    downloadDir = dir;
    notifyListeners();
  }

  void setActivePairing(PairingSessionView? s) {
    activePairing = s;
    notifyListeners();
  }

  /// 搜索（面板文件页用）。
  List<ShelfEntry> search(String query) => searchEntries(entries, query);

  @override
  void dispose() {
    _summaryTimer?.cancel();
    _noticeTimer?.cancel();
    super.dispose();
  }
}