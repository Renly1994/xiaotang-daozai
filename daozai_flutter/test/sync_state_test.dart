//! 同步状态回归测试 —— 摘要计数/红条可见与清除/进度/配对事件（R7.1 不静默）。
import 'package:flutter_test/flutter_test.dart';
import 'package:dynamic_island/core/pairing.dart';
import 'package:dynamic_island/core/shelf_index.dart';
import 'package:dynamic_island/services/sync_engine.dart';
import 'package:dynamic_island/state/sync_state.dart';

void main() {
  late SyncState s;
  setUp(() => s = SyncState());
  tearDown(() => s.dispose());

  test('收纳摘要：成功/重复/失败/跳过计数与文案', () {
    expect(IntakeSummary(added: 2, duplicate: 1).message, '已添加 2 个，1 个已存在');
    expect(IntakeSummary(duplicate: 1).message, '该文件已在置物架中');
    expect(IntakeSummary(duplicate: 3).message, '这些文件已在置物架中');
    expect(IntakeSummary(failed: 2).message, '文件无法添加');
    expect(IntakeSummary(added: 1, failed: 1, skipped: 2).message, '已添加 1 个，2 个跳过，1 个失败');
    s.showIntakeSummary(IntakeSummary(added: 1));
    expect(s.intakeSummary, isNotNull);
  });

  test('红条：失败可见；clearNotice 清除；新通知覆盖旧', () {
    s.onNotice('传输失败');
    expect(s.notice, '传输失败');
    s.onNotice('磁盘不足');
    expect(s.notice, '磁盘不足');
    s.clearNotice();
    expect(s.notice, isNull);
  });

  test('传输进度更新与完成移除', () {
    s.onTransferProgress(TransferProgress('sha', 50, 100));
    expect(s.transfers['sha']!.fraction, 0.5);
    s.onTransferDone('sha', '/tmp/x');
    expect(s.transfers.containsKey('sha'), isFalse);
  });

  test('设备在线/离线状态与最后在线时间', () {
    s.onPeerStatus('fp1', 'Mac', true);
    expect(s.peers['fp1']!.online, isTrue);
    s.onPeerStatus('fp1', 'Mac', false);
    expect(s.peers['fp1']!.online, isFalse);
    expect(s.peers['fp1']!.lastSeen, isNotNull);
  });

  test('配对失败 → 红条 + activePairing 清空；成功同样清空', () {
    final fail = PairingSession(token: 't', createdAt: DateTime.now());
    fail.startJoin();
    fail.onKeyexDone('111111', 'B', 'fpB');
    fail.confirm('000000'); // 错码 → failed
    s.onPairingUpdate(fail);
    expect(s.activePairing, isNull);
    expect(s.notice, contains('不一致'));
    final ok = PairingSession(token: 't2', createdAt: DateTime.now());
    ok.startJoin();
    ok.onKeyexDone('222222', 'C', 'fpC');
    ok.confirm('222222');
    ok.onRemoteConfirm(true);
    s.onPairingUpdate(ok);
    expect(s.activePairing, isNull);
  });

  test('投放态切换去重通知', () {
    var count = 0;
    s.addListener(() => count++);
    s.setDropHot(true);
    s.setDropHot(true);
    expect(count, 1);
    s.setDropHot(false);
    expect(count, 2);
  });

  test('搜索委托 shelf_index：大小写/中文', () {
    s.onEntriesChanged([
      ShelfEntry(id: '1', sha256: 'a', name: '报告.PDF', relPath: '报告.PDF', size: 1, modified: DateTime(2026), originDevice: 'x', addedAt: DateTime(2026)),
    ]);
    expect(s.search('pdf').length, 1);
    expect(s.search('报告').length, 1);
    expect(s.search('无').length, 0);
  });
}