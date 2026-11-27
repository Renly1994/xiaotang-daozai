//! 维吾尔语等阿拉伯字母文字的显示转换（#200）。
//!
//! 界面的文字排版（egui）不做字形连写，也不处理从右往左的方向：维语会显示成一个个孤立字母、顺序还是反的。
//! 这里在交给 egui 之前做两步，只影响显示，不改歌词原文：
//! 1. 连写：按前后字母把每个字母换成 Unicode「阿拉伯字母表现形式」里的词首 / 词中 / 词尾 / 独立形式，
//!    「ل + ا」合成一个字形。系统自带的 Segoe UI（界面首选字体）含全部维语字母的这些形式。
//! 2. 方向：按简化的双向文字规则排出显示顺序——阿拉伯字母从右往左，夹在其中的数字、英文、中文保持从左往右，
//!    括号在从右往左的段落里镜像。
//! 不含阿拉伯字母的文字原样返回，中文、英文、韩文、藏文都不受影响。
use std::borrow::Cow;

/// 字母的连写形式：(独立, 词尾, 词首, 词中)。词首 / 词中为 0 表示只向右连（如 ا د ر و），不与后一个字母相连。
fn forms(c: char) -> Option<[u32; 4]> {
    Some(match c as u32 {
        0x0621 => [0xFE80, 0, 0, 0],
        0x0622 => [0xFE81, 0xFE82, 0, 0],
        0x0623 => [0xFE83, 0xFE84, 0, 0],
        0x0624 => [0xFE85, 0xFE86, 0, 0],
        0x0625 => [0xFE87, 0xFE88, 0, 0],
        0x0626 => [0xFE89, 0xFE8A, 0xFE8B, 0xFE8C],
        0x0627 => [0xFE8D, 0xFE8E, 0, 0],
        0x0628 => [0xFE8F, 0xFE90, 0xFE91, 0xFE92],
        0x0629 => [0xFE93, 0xFE94, 0, 0],
        0x062A => [0xFE95, 0xFE96, 0xFE97, 0xFE98],
        0x062B => [0xFE99, 0xFE9A, 0xFE9B, 0xFE9C],
        0x062C => [0xFE9D, 0xFE9E, 0xFE9F, 0xFEA0],
        0x062D => [0xFEA1, 0xFEA2, 0xFEA3, 0xFEA4],
        0x062E => [0xFEA5, 0xFEA6, 0xFEA7, 0xFEA8],
        0x062F => [0xFEA9, 0xFEAA, 0, 0],
        0x0630 => [0xFEAB, 0xFEAC, 0, 0],
        0x0631 => [0xFEAD, 0xFEAE, 0, 0],
        0x0632 => [0xFEAF, 0xFEB0, 0, 0],
        0x0633 => [0xFEB1, 0xFEB2, 0xFEB3, 0xFEB4],
        0x0634 => [0xFEB5, 0xFEB6, 0xFEB7, 0xFEB8],
        0x0635 => [0xFEB9, 0xFEBA, 0xFEBB, 0xFEBC],
        0x0636 => [0xFEBD, 0xFEBE, 0xFEBF, 0xFEC0],
        0x0637 => [0xFEC1, 0xFEC2, 0xFEC3, 0xFEC4],
        0x0638 => [0xFEC5, 0xFEC6, 0xFEC7, 0xFEC8],
        0x0639 => [0xFEC9, 0xFECA, 0xFECB, 0xFECC],
        0x063A => [0xFECD, 0xFECE, 0xFECF, 0xFED0],
        0x0641 => [0xFED1, 0xFED2, 0xFED3, 0xFED4],
        0x0642 => [0xFED5, 0xFED6, 0xFED7, 0xFED8],
        0x0643 => [0xFED9, 0xFEDA, 0xFEDB, 0xFEDC],
        0x0644 => [0xFEDD, 0xFEDE, 0xFEDF, 0xFEE0],
        0x0645 => [0xFEE1, 0xFEE2, 0xFEE3, 0xFEE4],
        0x0646 => [0xFEE5, 0xFEE6, 0xFEE7, 0xFEE8],
        0x0647 => [0xFEE9, 0xFEEA, 0xFEEB, 0xFEEC],
        0x0648 => [0xFEED, 0xFEEE, 0, 0],
        // ى：维语里前后都连，用维语 / 哈萨克语专用的词首、词中形式。
        0x0649 => [0xFEEF, 0xFEF0, 0xFBE8, 0xFBE9],
        0x064A => [0xFEF1, 0xFEF2, 0xFEF3, 0xFEF4],
        0x067E => [0xFB56, 0xFB57, 0xFB58, 0xFB59], // پ
        0x0686 => [0xFB7A, 0xFB7B, 0xFB7C, 0xFB7D], // چ
        0x0698 => [0xFB8A, 0xFB8B, 0, 0],           // ژ
        0x06A9 => [0xFB8E, 0xFB8F, 0xFB90, 0xFB91], // ک
        0x06AD => [0xFBD3, 0xFBD4, 0xFBD5, 0xFBD6], // ڭ
        0x06AF => [0xFB92, 0xFB93, 0xFB94, 0xFB95], // گ
        0x06BE => [0xFBAA, 0xFBAB, 0xFBAC, 0xFBAD], // ھ
        0x06C6 => [0xFBD9, 0xFBDA, 0, 0],           // ۆ
        0x06C7 => [0xFBD7, 0xFBD8, 0, 0],           // ۇ
        0x06C8 => [0xFBDB, 0xFBDC, 0, 0],           // ۈ
        0x06CB => [0xFBDE, 0xFBDF, 0, 0],           // ۋ
        0x06CC => [0xFBFC, 0xFBFD, 0xFBFE, 0xFBFF], // ی
        0x06D0 => [0xFBE4, 0xFBE5, 0xFBE6, 0xFBE7], // ې
        // ە：维语只向右连，词尾写法与 ه 的词尾形式相同。
        0x06D5 => [0x06D5, 0xFEEA, 0, 0],
        _ => return None,
    })
}

/// 不占位置的附加符号（阿拉伯语注音符号等）：连写时跳过，排序时跟着前一个字母走。
fn transparent(c: char) -> bool {
    matches!(c as u32, 0x0610..=0x061A | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7..=0x06E8 | 0x06EA..=0x06ED)
}

/// 能和前后字母相连的「连接符」：阿拉伯延长线、零宽连接符。
fn join_causing(c: char) -> bool {
    matches!(c, '\u{0640}' | '\u{200D}')
}

fn joins_next(c: char) -> bool {
    join_causing(c) || forms(c).is_some_and(|f| f[2] != 0)
}

fn joins_prev(c: char) -> bool {
    join_causing(c) || forms(c).is_some_and(|f| f[1] != 0)
}

fn lam_alef(alef: char) -> Option<(u32, u32)> {
    Some(match alef as u32 {
        0x0622 => (0xFEF5, 0xFEF6),
        0x0623 => (0xFEF7, 0xFEF8),
        0x0625 => (0xFEF9, 0xFEFA),
        0x0627 => (0xFEFB, 0xFEFC),
        _ => return None,
    })
}

/// 第一步：按前后字母选连写形式（仍是阅读顺序）。
fn shape(text: &[char]) -> Vec<char> {
    let neighbour = |from: usize, step: isize| -> Option<char> {
        let mut i = from as isize + step;
        while i >= 0 && (i as usize) < text.len() {
            let c = text[i as usize];
            if !transparent(c) { return Some(c); }
            i += step;
        }
        None
    };
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        let Some(f) = forms(c) else { out.push(c); i += 1; continue; };
        let prev = neighbour(i, -1).is_some_and(joins_next);
        // ل + ا 合成一个字形（中间允许夹注音符号）。
        if c == '\u{0644}' {
            let next = (i + 1..text.len()).find(|&j| !transparent(text[j]));
            if let Some((j, (iso, fin))) = next.and_then(|j| lam_alef(text[j]).map(|forms| (j, forms))) {
                out.push(char::from_u32(if prev { fin } else { iso }).unwrap_or(c));
                out.extend(text[i + 1..j].iter().copied());
                i = j + 1;
                continue;
            }
        }
        let next = f[2] != 0 && neighbour(i, 1).is_some_and(joins_prev);
        let code = match (prev && f[1] != 0, next) {
            (true, true) => f[3],
            (true, false) => f[1],
            (false, true) => f[2],
            (false, false) => f[0],
        };
        out.push(char::from_u32(code).unwrap_or(c));
        i += 1;
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Class { L, R, Number, Neutral }

fn class(c: char) -> Class {
    match c as u32 {
        0x0030..=0x0039 | 0x0660..=0x0669 | 0x06F0..=0x06F9 => Class::Number,
        0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFC => Class::R,
        _ if c.is_alphabetic() => Class::L,
        _ => Class::Neutral,
    }
}

fn is_rtl_char(c: char) -> bool { class(c) == Class::R && !transparent(c) }

/// 是否含有需要从右往左显示的文字（只有这样的文字才做转换）。
pub fn has_rtl(text: &str) -> bool { text.chars().any(is_rtl_char) }

fn mirror(c: char) -> char {
    match c {
        '(' => ')', ')' => '(', '[' => ']', ']' => '[', '{' => '}', '}' => '{',
        '<' => '>', '>' => '<', '«' => '»', '»' => '«', '‹' => '›', '›' => '‹',
        _ => c,
    }
}

/// 第二步：一行文字的显示顺序（简化的 Unicode 双向文字规则）。
fn reorder(text: &[char]) -> String {
    // 附加符号和前一个字母合成一组，排序时整组移动，不会跑到别的字母上。
    let mut clusters: Vec<Vec<char>> = Vec::new();
    for &c in text {
        match clusters.last_mut() {
            Some(last) if transparent(c) => last.push(c),
            _ => clusters.push(vec![c]),
        }
    }
    let classes: Vec<Class> = clusters.iter().map(|cluster| class(cluster[0])).collect();
    // 段落方向：第一个有方向的字决定。
    let base: u8 = match classes.iter().find(|c| matches!(c, Class::L | Class::R)) { Some(Class::R) => 1, _ => 0 };
    // 判断中性字符两侧方向时，数字算作从右往左（与 Unicode 规则一致）。
    let strong = |c: Class| match c { Class::L => Some(false), Class::R | Class::Number => Some(true), Class::Neutral => None };
    let edge = base == 1;
    let mut levels = vec![base; clusters.len()];
    for i in 0..clusters.len() {
        levels[i] = match classes[i] {
            Class::R => 1,
            Class::L => if base == 1 { 2 } else { 0 },
            Class::Number => {
                let after_rtl = classes[..i].iter().rev().find_map(|c| match c { Class::L => Some(false), Class::R => Some(true), _ => None }).unwrap_or(edge);
                if base == 1 || after_rtl { 2 } else { 0 }
            }
            Class::Neutral => {
                let before = classes[..i].iter().rev().find_map(|c| strong(*c)).unwrap_or(edge);
                let after = classes[i + 1..].iter().find_map(|c| strong(*c)).unwrap_or(edge);
                match (before, after) {
                    (true, true) => 1,
                    (false, false) => if base == 1 { 2 } else { 0 },
                    _ => base,
                }
            }
        };
    }
    // 行尾空白回到段落方向。
    for i in (0..clusters.len()).rev() {
        if classes[i] != Class::Neutral || !clusters[i][0].is_whitespace() { break; }
        levels[i] = base;
    }
    let mut order: Vec<usize> = (0..clusters.len()).collect();
    let highest = levels.iter().copied().max().unwrap_or(0);
    for level in (1..=highest).rev() {
        let mut i = 0;
        while i < order.len() {
            if levels[order[i]] >= level {
                let start = i;
                while i < order.len() && levels[order[i]] >= level { i += 1; }
                order[start..i].reverse();
            } else {
                i += 1;
            }
        }
    }
    order.into_iter().flat_map(|i| {
        let rtl = levels[i] % 2 == 1;
        clusters[i].iter().map(move |&c| if rtl { mirror(c) } else { c }).collect::<Vec<_>>()
    }).collect()
}

/// 测试导出「修复前」对照图时关闭转换。
#[cfg(test)]
pub(crate) static BYPASS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 转换成显示用的文字；多行按行分别处理。不含从右往左文字时原样返回、不分配内存。
pub fn display(text: &str) -> Cow<'_, str> {
    #[cfg(test)]
    if BYPASS.load(std::sync::atomic::Ordering::Relaxed) { return Cow::Borrowed(text); }
    if !has_rtl(text) { return Cow::Borrowed(text); }
    Cow::Owned(text.split('\n').map(|line| {
        let chars: Vec<char> = line.chars().collect();
        reorder(&shape(&chars))
    }).collect::<Vec<_>>().join("\n"))
}

/// 视觉显示顺序中的单个字符到原始逻辑字符串的映射，供卡拉 OK 逐字染色按阅读顺序着色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualChar {
    /// 原始逻辑文本中的起始字符索引（按 `char` 计数）。
    pub logical_start: usize,
    /// 对应的逻辑字符数（普通字符为 1，ل+ا 合字为 2）。
    pub logical_len: usize,
    /// 该字符在当前双向 run 中是否按从右往左书写。
    pub rtl: bool,
}

/// 第一步（带映射）：按前后字母选连写形式，保留每个输出字符对应的原始逻辑范围。
fn shape_with_map(text: &[char], base_logical: usize) -> Vec<(char, usize, usize)> {
    let neighbour = |from: usize, step: isize| -> Option<char> {
        let mut i = from as isize + step;
        while i >= 0 && (i as usize) < text.len() {
            let c = text[i as usize];
            if !transparent(c) { return Some(c); }
            i += step;
        }
        None
    };
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        let Some(f) = forms(c) else { out.push((c, base_logical + i, 1)); i += 1; continue; };
        let prev = neighbour(i, -1).is_some_and(joins_next);
        // ل + ا 合成一个字形（中间允许夹注音符号）。
        if c == '\u{0644}' {
            let next = (i + 1..text.len()).find(|&j| !transparent(text[j]));
            if let Some((j, (iso, fin))) = next.and_then(|j| lam_alef(text[j]).map(|forms| (j, forms))) {
                let merged = char::from_u32(if prev { fin } else { iso }).unwrap_or(c);
                let span = (j - i) + 1;
                out.push((merged, base_logical + i, span));
                for k in (i + 1)..j {
                    out.push((text[k], base_logical + k, 1));
                }
                i = j + 1;
                continue;
            }
        }
        let next = f[2] != 0 && neighbour(i, 1).is_some_and(joins_prev);
        let code = match (prev && f[1] != 0, next) {
            (true, true) => f[3],
            (true, false) => f[1],
            (false, true) => f[2],
            (false, false) => f[0],
        };
        out.push((char::from_u32(code).unwrap_or(c), base_logical + i, 1));
        i += 1;
    }
    out
}

/// 第二步（带映射）：一行文字的显示顺序（与 `reorder` 完全一致的双向规则），
/// 返回显示字符串和每个视觉字符的 `VisualChar` 映射。
fn reorder_with_map(shaped: &[(char, usize, usize)]) -> (String, Vec<VisualChar>) {
    let mut clusters: Vec<Vec<(char, usize, usize)>> = Vec::new();
    for &item in shaped {
        match clusters.last_mut() {
            Some(last) if transparent(item.0) => last.push(item),
            _ => clusters.push(vec![item]),
        }
    }
    let classes: Vec<Class> = clusters.iter().map(|cluster| class(cluster[0].0)).collect();
    let base: u8 = match classes.iter().find(|c| matches!(c, Class::L | Class::R)) { Some(Class::R) => 1, _ => 0 };
    let strong = |c: Class| match c { Class::L => Some(false), Class::R | Class::Number => Some(true), Class::Neutral => None };
    let edge = base == 1;
    let mut levels = vec![base; clusters.len()];
    for i in 0..clusters.len() {
        levels[i] = match classes[i] {
            Class::R => 1,
            Class::L => if base == 1 { 2 } else { 0 },
            Class::Number => {
                let after_rtl = classes[..i].iter().rev().find_map(|c| match c { Class::L => Some(false), Class::R => Some(true), _ => None }).unwrap_or(edge);
                if base == 1 || after_rtl { 2 } else { 0 }
            }
            Class::Neutral => {
                let before = classes[..i].iter().rev().find_map(|c| strong(*c)).unwrap_or(edge);
                let after = classes[i + 1..].iter().find_map(|c| strong(*c)).unwrap_or(edge);
                match (before, after) {
                    (true, true) => 1,
                    (false, false) => if base == 1 { 2 } else { 0 },
                    _ => base,
                }
            }
        };
    }
    for i in (0..clusters.len()).rev() {
        if classes[i] != Class::Neutral || !clusters[i][0].0.is_whitespace() { break; }
        levels[i] = base;
    }
    let mut order: Vec<usize> = (0..clusters.len()).collect();
    let highest = levels.iter().copied().max().unwrap_or(0);
    for level in (1..=highest).rev() {
        let mut i = 0;
        while i < order.len() {
            if levels[order[i]] >= level {
                let start = i;
                while i < order.len() && levels[order[i]] >= level { i += 1; }
                order[start..i].reverse();
            } else {
                i += 1;
            }
        }
    }
    let mut text = String::with_capacity(shaped.len() * 3);
    let mut map = Vec::with_capacity(shaped.len());
    for i in order {
        let rtl = levels[i] % 2 == 1;
        for &(c, logical_start, logical_len) in &clusters[i] {
            text.push(if rtl { mirror(c) } else { c });
            map.push(VisualChar { logical_start, logical_len, rtl });
        }
    }
    (text, map)
}

/// 对含双向文字的歌词按与 `display_wrapped` 相同的规则分行并转换，
/// 同时返回每行非换行视觉字符的逻辑映射（不含 RTL 文字时返回 `None`，走零分配快速路径）。
pub fn display_wrapped_with_map(text: &str, max_width: f32, width: impl Fn(&str) -> f32) -> (String, Option<Vec<VisualChar>>) {
    #[cfg(test)]
    if BYPASS.load(std::sync::atomic::Ordering::Relaxed) {
        return (text.to_owned(), None);
    }
    if !has_rtl(text) {
        return (text.to_owned(), None);
    }
    // 按空格记录每个词在原逻辑文本中的字符起止位置，确保折行后映射与原文完全对齐。
    let mut visual_lines: Vec<String> = Vec::new();
    let mut visual_map: Vec<VisualChar> = Vec::new();
    let mut logical_cursor = 0usize;
    for (p_idx, paragraph) in text.split('\n').enumerate() {
        if p_idx > 0 {
            logical_cursor += 1; // '\n'
        }
        let p_chars: Vec<char> = paragraph.chars().collect();
        let mut word_spans: Vec<(usize, usize)> = Vec::new();
        let mut i = 0usize;
        while i < p_chars.len() {
            while i < p_chars.len() && p_chars[i] == ' ' { i += 1; }
            let start = i;
            while i < p_chars.len() && p_chars[i] != ' ' { i += 1; }
            if i > start {
                word_spans.push((start, i));
            }
        }
        let mut line_words: Vec<(usize, usize)> = Vec::new();
        let mut current = String::new();
        let flush_line = |line_words: &[(usize, usize)], visual_lines: &mut Vec<String>, visual_map: &mut Vec<VisualChar>| {
            if line_words.is_empty() {
                visual_lines.push(String::new());
                return;
            }
            let mut chars = Vec::new();
            let mut logical_indices = Vec::new();
            for (idx, &(ws, we)) in line_words.iter().enumerate() {
                if idx > 0 {
                    // 取该词前紧邻的一个空格作为视觉空格的逻辑索引。
                    let space_idx = ws.saturating_sub(1);
                    chars.push(' ');
                    logical_indices.push(logical_cursor + space_idx);
                }
                for pos in ws..we {
                    chars.push(p_chars[pos]);
                    logical_indices.push(logical_cursor + pos);
                }
            }
            // 将 shape_with_map 输出的相对位置还原为真实逻辑索引。
            let raw_shaped = shape_with_map(&chars, 0);
            let mapped_shaped: Vec<(char, usize, usize)> = raw_shaped
                .into_iter()
                .map(|(ch, rel_start, len)| {
                    let start = logical_indices.get(rel_start).copied().unwrap_or(logical_cursor);
                    (ch, start, len)
                })
                .collect();
            let (vis, map) = reorder_with_map(&mapped_shaped);
            visual_lines.push(vis);
            visual_map.extend(map);
        };
        for &(ws, we) in &word_spans {
            let word: String = p_chars[ws..we].iter().collect();
            let candidate = if current.is_empty() { word.clone() } else { format!("{current} {word}") };
            if !current.is_empty() && width(&display(&candidate)) > max_width {
                flush_line(&line_words, &mut visual_lines, &mut visual_map);
                line_words.clear();
                current = word;
            } else {
                current = candidate;
            }
            line_words.push((ws, we));
        }
        flush_line(&line_words, &mut visual_lines, &mut visual_map);
        logical_cursor += p_chars.len();
    }
    (visual_lines.join("\n"), Some(visual_map))
}

/// 需要折行的文字（岛上歌词、歌词列表）：从右往左的句子必须先按阅读顺序分行、再逐行转换，
/// 否则交给 egui 折行时会先显示句子的结尾。按空格分词，`width` 量的是转换后的宽度。
/// 不含从右往左文字时原样返回，仍由 egui 自己折行。
pub fn display_wrapped(text: &str, max_width: f32, width: impl Fn(&str) -> f32) -> Cow<'_, str> {
    #[cfg(test)]
    if BYPASS.load(std::sync::atomic::Ordering::Relaxed) { return Cow::Borrowed(text); }
    if !has_rtl(text) { return Cow::Borrowed(text); }
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split(' ').filter(|w| !w.is_empty()) {
            let candidate = if current.is_empty() { word.to_owned() } else { format!("{current} {word}") };
            if !current.is_empty() && width(&display(&candidate)) > max_width {
                lines.push(std::mem::replace(&mut current, word.to_owned()));
            } else {
                current = candidate;
            }
        }
        lines.push(current);
    }
    Cow::Owned(lines.iter().map(|line| display(line).into_owned()).collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(text: &str) -> Vec<u32> { text.chars().map(|c| c as u32).collect() }

    #[test]
    fn text_without_arabic_script_is_untouched() {
        for text in ["晴天 - 周杰伦", "Hello (World) 123", "사랑해", "བཀྲ་ཤིས་བདེ་ལེགས།", ""] {
            assert!(matches!(display(text), Cow::Borrowed(_)), "{text}");
        }
    }

    #[test]
    fn uyghur_letters_join_and_read_right_to_left() {
        // سىنى（你）：س 词首、ى 词中（维语专用形式）、ن 词中、ى 词尾；显示顺序从右往左，所以倒过来。
        let shown = display("سىنى");
        assert_eq!(codes(&shown), vec![0xFEF0, 0xFEE8, 0xFBE9, 0xFEB3]);
        // ئۈ：ئ 词首，ۈ 只向右连，取词尾形式。
        assert_eq!(codes(&display("ئۈ")), vec![0xFBDC, 0xFE8B]);
        // 只向右连的字母后面另起：دا → د 独立、ا 独立。
        assert_eq!(codes(&display("دا")), vec![0xFE8D, 0xFEA9]);
    }

    #[test]
    fn lam_alef_becomes_one_ligature() {
        assert_eq!(codes(&display("لا")), vec![0xFEFB]);
        // 前面有相连的字母时用词尾合体形式：بلا → ب 词首 + لا 词尾合体。
        assert_eq!(codes(&display("بلا")), vec![0xFEFC, 0xFE91]);
    }

    #[test]
    fn words_keep_their_order_from_right_to_left() {
        // 两个词：第一个词显示在右边。
        let shown: Vec<char> = display("سىز مەن").chars().collect();
        let space = shown.iter().position(|c| *c == ' ').unwrap();
        // مەن 的 ن 在只向右连的 ە 之后，是独立形式，显示在最左边；سىز 的 س 是词首，显示在最右边。
        assert_eq!(shown[0] as u32, 0xFEE5);
        assert_eq!(*shown.last().unwrap() as u32, 0xFEB3);
        assert!(space > 0 && space < shown.len() - 1);
    }

    #[test]
    fn numbers_and_latin_stay_left_to_right_inside_uyghur() {
        let shown = display("ناخشا 2024 Song");
        assert!(shown.contains("2024"), "数字保持从左往右：{shown}");
        assert!(shown.contains("Song"), "英文保持从左往右：{shown}");
        // 段落从右往左：英文在数字左边，维语词在最右边。
        let song = shown.find("Song").unwrap();
        let year = shown.find("2024").unwrap();
        assert!(song < year, "{shown}");
    }

    #[test]
    fn brackets_are_mirrored_in_right_to_left_text() {
        // 阅读顺序「歌名（歌词）」从右往左显示：括号内容在左侧，镜像后最左边是「(」、括号内容右边是「)」。
        let shown = display("ناخشا (سۆز)");
        let open = shown.find('(').unwrap();
        let close = shown.find(')').unwrap();
        assert_eq!(open, 0, "{shown}");
        assert!(close > open && shown[close..].contains(' '), "括号闭合在维语词之前：{shown}");
    }

    #[test]
    fn chinese_line_with_an_uyghur_word_keeps_chinese_order() {
        let shown = display("歌曲 ناخشا 好听");
        assert!(shown.starts_with("歌曲 ") && shown.ends_with(" 好听"), "段落以中文开头时仍从左往右：{shown}");
    }

    #[test]
    fn truncation_ellipsis_ends_up_on_the_left() {
        // 截断按阅读顺序在末尾加省略号；从右往左显示时，阅读的末尾在最左边。
        let shown = display("سۆيگۈ دېگەن شۇنداق…");
        assert!(shown.starts_with('…'), "{shown}");
    }

    #[test]
    fn marks_stay_with_their_letter() {
        // بَ：注音符号紧跟在字母后面，反转后也不能跑到另一个字母上。
        let shown: Vec<char> = display("بَت").chars().collect();
        let mark = shown.iter().position(|c| *c == '\u{064E}').unwrap();
        assert_eq!(shown[mark - 1] as u32, 0xFE91, "符号紧跟在 ب 后面：{shown:?}");
    }

    #[test]
    fn wrapping_splits_by_reading_order_before_reversing() {
        // 每个词算 10 宽，一行最多 25：前两个词一行、第三个词下一行。
        let width = |s: &str| s.split(' ').count() as f32 * 10.0;
        let shown = display_wrapped("بىر ئىككى ئۈچ", 25.0, width);
        let lines: Vec<&str> = shown.split('\n').collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], display("بىر ئىككى"), "第一行是句子开头的两个词");
        assert_eq!(lines[1], display("ئۈچ"));
        assert!(matches!(display_wrapped("中文歌词", 10.0, width), Cow::Borrowed(_)));
    }
}
