//! 文件页/设备页 Widget 测试 —— 搜索过滤/空状态/三态图标/红条/下载位置（验收 13、R5.6）。
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:dynamic_island/core/shelf_index.dart';
import 'package:dynamic_island/core/theme.dart';
import 'package:dynamic_island/state/app_state.dart';
import 'package:dynamic_island/state/sync_state.dart';
import 'package:dynamic_island/ui/files_page.dart';
import 'package:dynamic_island/ui/devices_page.dart';

ShelfEntry e(String id, String name, {EntryLocation loc = EntryLocation.local, String? remote}) =>
    ShelfEntry(
      id: id, sha256: 'sha$id', name: name, relPath: name, size: 10,
      modified: DateTime(2026), originDevice: 'fpA', addedAt: DateTime(2026),
      location: loc, remoteDevice: remote,
    );

Widget wrap(SyncState sync, Widget child) => MultiProvider(
      providers: [
        ChangeNotifierProvider<AppState>(create: (_) => AppState()),
        ChangeNotifierProvider<SyncState>.value(value: sync),
      ],
      child: MaterialApp(
        theme: buildDaozaiTheme(),
        home: Scaffold(body: SizedBox(width: 419, height: 140, child: child)),
      ),
    );

void main() {
  late SyncState sync;
  setUp(() => sync = SyncState());
  tearDown(() => sync.dispose());

  testWidgets('空置物架显示引导文案（R5.6）', (tester) async {
    await tester.pumpWidget(wrap(sync, const FilesPage()));
    expect(find.text('拖文件到岛体，或点 + 添加'), findsOneWidget);
  });

  testWidgets('搜索实时过滤：大小写不敏感、中文子串（验收 13）', (tester) async {
    sync.onEntriesChanged([e('1', '年度报告.PDF'), e('2', 'photo.jpg')]);
    await tester.pumpWidget(wrap(sync, const FilesPage()));
    expect(find.text('年度报告.PDF'), findsOneWidget);
    await tester.enterText(find.byType(TextField), 'pdf');
    await tester.pump();
    expect(find.text('年度报告.PDF'), findsOneWidget);
    expect(find.text('photo.jpg'), findsNothing);
    await tester.enterText(find.byType(TextField), '年度');
    await tester.pump();
    expect(find.text('年度报告.PDF'), findsOneWidget);
    await tester.enterText(find.byType(TextField), '不存在');
    await tester.pump();
    expect(find.text('无匹配文件'), findsOneWidget);
  });

  testWidgets('远端条目显示云图标+设备名；本地显示文件图标', (tester) async {
    sync.onEntriesChanged([
      e('1', 'local.txt'),
      e('2', 'remote.txt', loc: EntryLocation.remote, remote: 'fpB'),
    ]);
    sync.onPeerStatus('fpB', 'Win-B', true);
    await tester.pumpWidget(wrap(sync, const FilesPage()));
    expect(find.byIcon(Icons.insert_drive_file_outlined), findsOneWidget);
    expect(find.byIcon(Icons.cloud_outlined), findsOneWidget);
    expect(find.text('Win-B'), findsOneWidget);
  });

  testWidgets('收纳摘要与红条可见', (tester) async {
    sync.onEntriesChanged([e('1', 'a.txt')]);
    sync.showIntakeSummary(IntakeSummary(added: 1));
    sync.onNotice('磁盘空间不足');
    await tester.pumpWidget(wrap(sync, const FilesPage()));
    expect(find.textContaining('已添加 1'), findsOneWidget);
    expect(find.text('磁盘空间不足'), findsOneWidget);
    // 推进时间让摘要/红条自动清除定时器到期，避免 pending timer 断言。
    await tester.pump(const Duration(seconds: 9));
  });

  testWidgets('设备页：空列表占位+配对按钮+同步开关+下载位置', (tester) async {
    await tester.pumpWidget(wrap(sync, const DevicesPage()));
    expect(find.text('暂无已配对设备'), findsOneWidget);
    expect(find.text('发起配对'), findsOneWidget);
    expect(find.text('加入配对'), findsOneWidget);
    expect(find.byType(Switch), findsOneWidget);
    expect(find.textContaining('下载位置'), findsOneWidget);
  });

  testWidgets('设备页：在线/离线状态与移除二次确认', (tester) async {
    sync.onPeerStatus('fpB', 'Win-B', true);
    await tester.pumpWidget(wrap(sync, const DevicesPage()));
    expect(find.text('在线'), findsOneWidget);
    // 第一次点 × 进入确认态，不立即删除。
    await tester.tap(find.byIcon(Icons.close));
    await tester.pump();
    expect(find.byIcon(Icons.delete_forever), findsOneWidget);
    expect(sync.peers.containsKey('fpB'), isTrue);
    sync.onPeerStatus('fpB', 'Win-B', false);
    await tester.pump();
    expect(find.text('离线'), findsOneWidget);
  });
}