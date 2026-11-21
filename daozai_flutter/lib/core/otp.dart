//! 验证码提取 —— 忠实复刻原项目 src/otp.rs 的加权打分。
//!
//! 加权打分而不是单条正则：单正则会把你订单号、金额、电话当验证码。
//! 门槛 65 分——宁可漏，不可错。

class OtpCode {
  final String text;
  final int score;
  const OtpCode(this.text, this.score);
}

const int kOtpThreshold = 65;

final RegExp _strong = RegExp(
  r'[验驗][证證][码碼]|校[验驗][码碼]|[确確][认認][码碼]|[动動][态態](?:密)?[码碼]|安全[码碼]|登[录錄入][码碼]|激活[码碼]|'
  r'一次性(?:密[码碼]|口令)|otp|one[-\s]?time\s+(?:code|password|passcode)|'
  r'verification\s+code|security\s+code|auth(?:entication)?\s+code|'
  r'passcode|\bcode\b|認証コード|인증번호',
  caseSensitive: false,
);

final RegExp _weak = RegExp(
  r'登[录錄入陆陸]|[验驗][证證]|校[验驗]|身份|[确確][认認]|安全|'
  r'sign[-\s]?in|log[-\s]?in|verify|verification|confirm|authenticate',
  caseSensitive: false,
);

final RegExp _candidate = RegExp(
  r'(?:[A-Za-z]{1,3}-)?'
  r'(?:\d{3,4}[- ]\d{3,4}|\d{4}-\d{4}|[A-Za-z0-9]{3,4}-[A-Za-z0-9]{3,4}|\d{4,8}|[A-Za-z0-9]{4,8})',
);

final RegExp _clock = RegExp(r'\d{1,2}:\d{2}');

const List<String> _orderWords = [
  '订单', '单号', '快递', '运单', '物流', '包裹', '编号', '序号', '发票', '金额', '合计',
  '实付', '应付', '电话', '手机号', '会议号', '会议', '卡尾号', '尾号', '邮编', 'sku',
  '訂單', '單號', '快遞', '運單', '編號', '序號', '發票', '金額', '合計', '實付', '應付', '電話', '手機號', '會議', '尾號', '郵編',
  'order', 'track', 'invoice', 'amount', 'phone', 'meeting', 'card ending',
];

const List<String> _moneyWords = [
  '¥', '￥', r'$', '€', '元', '价格', '金额', '合计', '实付', '应付', '價格', '金額', '合計', '實付', '應付',
];

const List<String> _promoWords = [
  '优惠', '折扣', '促销', '满减', '券', '红包', 'coupon', 'promo', 'discount', 'sale',
  '優惠', '促銷', '滿減', '紅包',
];

const List<String> _expireWords = [
  '有效', '过期', '失效', 'expire', 'valid', 'minutes', '分钟', '小时内', '過期', '分鐘', '小時內',
];

/// 全角转半角、去零宽字符、全角空格归一。
String _normalize(String s) {
  final out = StringBuffer();
  for (final r in s.runes) {
    if (r == 0x200b || r == 0x200c || r == 0x200d || r == 0xfeff || r == 0x2060) continue;
    if (r == 0x3000) {
      out.write(' ');
    } else if (r >= 0xff01 && r <= 0xff5e) {
      out.writeCharCode(r - 0xfee0);
    } else {
      out.writeCharCode(r);
    }
  }
  return out.toString();
}

String _takeChars(String s, int n) =>
    String.fromCharCodes(s.runes.take(n));

String _before(String text, int at, int n) {
  final chars = text.substring(0, at).runes.toList().reversed.take(n).toList();
  return String.fromCharCodes(chars.reversed);
}

String _after(String text, int at, int n) =>
    String.fromCharCodes(text.substring(at).runes.take(n));

String _around(String text, int start, int end, int n) =>
    '${_before(text, start, n)}${_after(text, end, n)}';

bool _has(String win, List<String> words) {
  final lower = win.toLowerCase();
  return words.any((w) => lower.contains(w));
}

bool _isAsciiAlnum(int r) =>
    (r >= 0x30 && r <= 0x39) || (r >= 0x41 && r <= 0x5a) || (r >= 0x61 && r <= 0x7a);

/// 从主题 + 正文里挑出最像验证码的一个；[emphasized] 是 HTML 里加粗/大字号片段的纯文本。
OtpCode? extractOtp(String subject, String body, List<String> emphasized) {
  final subj = _normalize(subject);
  final bod = _normalize(_takeChars(body, 6000));
  final hay = '$subj\n$bod';
  final subjectEnd = subj.runes.length + 1;
  final emph = emphasized.map(_normalize).toList();

  int? bestScore;
  int? bestPos;
  bool bestInSubject = false;
  OtpCode? best;

  for (final m in _candidate.allMatches(hay)) {
    final start = m.start, end = m.end;
    final prev = start > 0 ? hay.codeUnitAt(start - 1) : null;
    final next = end < hay.length ? hay.codeUnitAt(end) : null;
    // 只认 ASCII 字母数字边界：中文紧贴验证码是常态，不能当边界外丢弃。
    if ((prev != null && _isAsciiAlnum(prev)) || (next != null && _isAsciiAlnum(next))) continue;

    final text = m.group(0)!;
    final core = text.replaceAll(RegExp(r'[^A-Za-z0-9]'), '');
    if (core.isEmpty) continue;
    final digits = core.runes.where((r) => r >= 0x30 && r <= 0x39).length;
    if (digits == 0) continue; // 纯字母分支骨架阶段从简，仅处理含数字码

    final result = _scoreAt(hay, start, end, text, subjectEnd, emph);
    if (result == null) continue;
    final (score, strongSignal) = result;
    // 没有强信号就不算验证码：宁可不显示，也不能把订单号、金额放大展示。
    if (!strongSignal) continue;

    final charPos = hay.substring(0, start).runes.length;
    final inSubject = charPos < subjectEnd;
    final better = best == null ||
        score > bestScore! ||
        (score == bestScore && inSubject && !bestInSubject) ||
        (score == bestScore && inSubject == bestInSubject && charPos < bestPos!);
    if (better) {
      bestScore = score;
      bestPos = charPos;
      bestInSubject = inSubject;
      best = OtpCode(text, score);
    }
  }
  if (best != null && best.score >= kOtpThreshold) return best;
  return null;
}

(int, bool)? _scoreAt(String hay, int start, int end, String text, int subjectEnd, List<String> emph) {
  final core = text.replaceAll(RegExp(r'[^A-Za-z0-9]'), '');
  final digits = core.runes.where((r) => r >= 0x30 && r <= 0x39).length;
  final letters = core.runes.where((r) => (r >= 0x41 && r <= 0x5a) || (r >= 0x61 && r <= 0x7a)).length;
  if (digits == 0) return null;
  final mixed = letters > 0;
  final separated = text.contains('-') || text.contains(' ');
  final len = core.runes.length;

  int score;
  if (mixed) {
    if (len < 5) return null;
    score = len == 6 ? 20 : (separated ? 22 : 14);
  } else {
    switch (len) {
      case 6: score = separated ? 28 : 30; break;
      case 7:
      case 8: score = separated ? 22 : 18; break;
      case 5: score = 20; break;
      case 4: score = 16; break;
      default: return null;
    }
  }

  final b40 = _before(hay, start, 40);
  final b250 = _before(hay, start, 250);
  final b60 = _before(hay, start, 60);
  final a40 = _after(hay, end, 40);

  // ---- 硬丢弃：URL / 邮箱 / 日期时间 / 电话 / 订单金额卡号 ----
  var left = 0;
  for (var i = start - 1; i >= 0; i--) {
    if (hay[i].trim().isEmpty) { left = i + 1; break; }
  }
  var right = hay.length;
  for (var i = end; i < hay.length; i++) {
    if (hay[i].trim().isEmpty) { right = i; break; }
  }
  final token = hay.substring(left, right);
  final tl = token.toLowerCase();
  if (tl.contains('http') || tl.contains('www.') || token.contains('/') || token.contains('@')) return null;

  final dateWin = _around(hay, start, end, 6);
  if (_has(dateWin, ['年', '月', '日', '有效期至']) ||
      _clock.hasMatch(dateWin) ||
      _clock.hasMatch(_before(hay, start, 8))) return null;
  if (_has(_around(hay, start, end, 6), _moneyWords)) return null;
  if (_has(_before(hay, start, 20), _orderWords) || _has(_after(hay, end, 8), _orderWords)) return null;

  // 4 位年份：直接判负。
  if (len == 4 && !mixed) {
    final y = int.tryParse(core);
    if (y != null && y >= 1900 && y <= 2100) score -= 35;
  }

  // ---- 正向信号 ----
  final strongBefore = _strong.hasMatch(b40);
  if (strongBefore) {
    score += 40;
  } else if (_strong.hasMatch(b250)) {
    score += 22;
  } else if (_weak.hasMatch(b60)) {
    score += 15;
  }
  if (_strong.hasMatch(a40)) score += 28;

  final lineStart = hay.lastIndexOf('\n', start == 0 ? 0 : start - 1) + 1;
  final nlAfter = hay.indexOf('\n', start);
  final lineEnd = nlAfter < 0 ? hay.length : nlAfter;
  final line = hay.substring(lineStart, lineEnd).trim().replaceAll(RegExp(r'[。，,.！!、;；]+$'), '').trim();
  final ownLine = line == text;
  final colon = hay.substring(0, start).trimRight().endsWith(':');
  final charPos = hay.substring(0, start).runes.length;
  final inSubject = charPos < subjectEnd;
  final isEmph = emph.any((e) => e.contains(text));

  if (mixed && !(ownLine || colon || inSubject || isEmph)) return null;

  if (ownLine) score += 15;
  if (colon) score += 10;
  if (inSubject) score += 15;
  if (isEmph) score += 25;
  if (_has(b40, ['验证码是', '验证码为', '校验码是', '驗證碼是', '驗證碼為', '校驗碼是', 'code is', 'code:'])) score += 5;
  if (_has(a40, _expireWords) || _has(b60, _expireWords)) score += 5;
  if (_has(_around(hay, start, end, 40), _promoWords)) score -= 30;

  final strongSignal = strongBefore || _strong.hasMatch(a40) || inSubject || isEmph || ownLine || colon;
  return (score, strongSignal);
}

/// HTML → 纯文本：去脚本样式、块级标签转换行、解常见实体、压缩空白。
String htmlToText(String html) {
  var s = html.replaceAll(RegExp(r'<(?:script|style)[^>]*>.*?</(?:script|style)>', caseSensitive: false, dotAll: true), '');
  s = s.replaceAll(RegExp(r'<(?:br|/p|/div|/li|/h[1-6]|/tr)[^>]*>', caseSensitive: false), '\n');
  s = s.replaceAll(RegExp(r'<[^>]+>', dotAll: true), '');
  s = s.replaceAll('&nbsp;', ' ').replaceAll('&amp;', '&').replaceAll('&lt;', '<')
      .replaceAll('&gt;', '>').replaceAll('&quot;', '"').replaceAll('&#39;', "'");
  s = s.replaceAll(RegExp(r'[ \t]+'), ' ');
  s = s.replaceAll(RegExp(r'\n\s*\n+'), '\n');
  return s.trim();
}