//! 置物架索引回归测试 —— 去重/搜索/幂等 upsert/JSON 往返（验收 1、3、13）。
import 'package:flutter_test/flutter_test.dart';
import 'package:dynamic_island/core/shelf_index.dart';

ShelfEntry entry(String sha, String path, {String? name, bool trashed = false, int version = 1}) =>
    ShelfEntry(
      id: '$sha-$path',
      sha256: sha,
      name: name ?? path.split('/').last,
      relPath: path,
      size: 10,
      modified: DateTime(2026, 1, 1),
      originDevice: 'devA',
      addedAt: DateTime(2026, 1, 1),
      trashed: trashed,
      version: version,
    );

void main() {
  test('去重：同内容同路径命中；同内容不同路径不误判', () {
    final list = [entry('aaa', 'docs/a.txt')];
    expect(isDuplicate(list, 'aaa', 'docs/a.txt'), isTrue);
    expect(isDuplicate(list, 'aaa', 'other/a.txt'), isFalse);
    expect(isDuplicate(list, 'bbb', 'docs/a.txt'), isFalse);
  });

  test('回收站条目不参与去重（恢复前可重新收纳）', () {
    final list = [entry('aaa', 'a.txt', trashed: true)];
    expect(isDuplicate(list, 'aaa', 'a.txt'), isFalse);
  });

  test('搜索：大小写不敏感、中文子串、路径匹配、空关键词全量', () {
    final list = [
      entry('1', '报告/年度总结.PDF'),
      entry('2', 'photos/IMG_001.jpg'),
      entry('3', 'notes.txt'),
    ];
    expect(searchEntries(list, 'pdf').length, 1);
    expect(searchEntries(list, '年度').single.relPath, '报告/年度总结.PDF');
    expect(searchEntries(list, 'img_001').length, 1);
    expect(searchEntries(list, '').length, 3);
    expect(searchEntries(list, '  ').length, 3);
    expect(searchEntries(list, '不存在'), isEmpty);
  });

  test('搜索排除回收站条目', () {
    final list = [entry('1', 'a.txt'), entry('2', 'b.txt', trashed: true)];
    expect(searchEntries(list, '').length, 1);
  });

  test('upsert 幂等：同键覆盖且保留更高 version；新键插首位', () {
    var list = [entry('aaa', 'a.txt', version: 1)];
    list = upsertEntry(list, entry('aaa', 'a.txt', version: 2));
    expect(list.length, 1);
    expect(list.single.version, 2);
    list = upsertEntry(list, entry('aaa', 'a.txt', version: 1));
    expect(list.single.version, 2); // 旧版本不回退
    list = upsertEntry(list, entry('bbb', 'b.txt'));
    expect(list.length, 2);
    expect(list.first.sha256, 'bbb');
  });

  test('JSON 往返全字段一致；损坏数据返回空列表', () {
    final list = [
      ShelfEntry(
        id: 'x', sha256: 's', name: 'n', relPath: 'd/n', size: 42,
        modified: DateTime(2026, 5, 1, 12, 30), originDevice: 'fp1',
        addedAt: DateTime(2026, 5, 2), location: EntryLocation.remote,
        remoteDevice: 'fp2', trashed: true, trashedAt: DateTime(2026, 5, 3), version: 7,
      ),
    ];
    final back = ShelfEntry.decodeList(ShelfEntry.encodeList(list));
    expect(back.single.toJson(), list.single.toJson());
    expect(ShelfEntry.decodeList('not json'), isEmpty);
  });
}