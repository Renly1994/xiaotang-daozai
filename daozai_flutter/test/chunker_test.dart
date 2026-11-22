//! 分块/校验回归测试 —— 边界分块/流式哈希/校验判定/路径安全化。
import 'package:flutter_test/flutter_test.dart';
import 'package:dynamic_island/core/chunker.dart';

void main() {
  test('分块：恰好整除无空块；余 1 字节成块；0/负数无块', () {
    expect(chunkPlan(1024, 256).length, 4);
    expect(chunkPlan(1024, 256).last.length, 256);
    final p = chunkPlan(1025, 256);
    expect(p.length, 5);
    expect(p.last.offset, 1024);
    expect(p.last.length, 1);
    expect(chunkPlan(0), isEmpty);
    expect(chunkPlan(-1), isEmpty);
    expect(chunkPlan(100, 256).single.length, 100);
  });

  test('分块覆盖全文件无重叠无缝隙', () {
    final p = chunkPlan(1000, 300);
    var expectOff = 0;
    for (final c in p) {
      expect(c.offset, expectOff);
      expectOff += c.length;
    }
    expect(expectOff, 1000);
  });

  test('流式 SHA-256 与一次性一致；resumeOffset 即已收字节', () {
    final bytes = List<int>.generate(1000, (i) => i % 251);
    final sink = Sha256Sink();
    sink.add(bytes.sublist(0, 300));
    sink.add(bytes.sublist(300, 999));
    sink.add(bytes.sublist(999));
    expect(sink.bytes, 1000);
    expect(sink.digest(), sha256Hex(bytes));
    expect(resumeOffset(300), 300);
  });

  test('verifyBlob：长度与哈希都一致才通过', () {
    expect(verifyBlob('a', 10, 'a', 10), isTrue);
    expect(verifyBlob('a', 10, 'a', 11), isFalse);
    expect(verifyBlob('a', 10, 'b', 10), isFalse);
  });

  test('sanitizeRelPath：防目录穿越/控制字符/反斜杠归一', () {
    expect(sanitizeRelPath('../../etc/passwd'), 'etc/passwd');
    expect(sanitizeRelPath('a\\b\\c.txt'), 'a/b/c.txt');
    expect(sanitizeRelPath('a//./b.txt'), 'a/b.txt');
    expect(sanitizeRelPath('a\x00b.txt'), 'ab.txt');
    expect(sanitizeRelPath('/绝对路径/x'), '绝对路径/x');
  });
}