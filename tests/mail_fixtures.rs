//! 邮件排版金标准回归：100 封合成语料（`tests/fixtures/mail/eml`）逐封走解析器，
//! 断言「HTML 结构特征 → 块结构」的映射没有被改坏。
//!
//! 语料按五种真实形态生成（A 通栏轻 HTML / B 居中单卡 CTA / C 深色 hero /
//! D 中文事务 / E 混排多栏），特征清单在 `manifest.json` 里。
//! 断言只依赖结构特征，不含任何真实邮箱、域名或发件人特判。

use daozai::mail_doc::{self as mail_format, Block, Doc};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mail")
}

fn manifest() -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(root().join("manifest.json")).expect("manifest.json 必须存在");
    serde_json::from_str(&text).expect("manifest.json 必须是合法 JSON")
}

/// 从 `.eml` 里取出 `text/html` 部分（真正的 MIME 解析，不靠正则猜）。
fn html_of(eml: &[u8]) -> String {
    fn walk(part: &mailparse::ParsedMail<'_>, out: &mut Option<String>) {
        if out.is_some() {
            return;
        }
        if part.ctype.mimetype == "text/html" {
            if let Ok(body) = part.get_body() {
                *out = Some(body);
                return;
            }
        }
        for sub in &part.subparts {
            walk(sub, out);
        }
    }
    let parsed = mailparse::parse_mail(eml).expect("语料必须是合法 MIME");
    let mut out = None;
    walk(&parsed, &mut out);
    out.unwrap_or_default()
}

fn doc_for(id: &str) -> Doc {
    let bytes = std::fs::read(root().join("eml").join(format!("{id}.eml"))).expect("语料文件必须存在");
    mail_format::render(&html_of(&bytes), "")
}

fn text_of(doc: &Doc) -> String {
    let mut out = String::new();
    for block in &doc.blocks {
        match block {
            Block::Paragraph(p) => out.push_str(&p.runs.iter().map(|r| r.text.as_str()).collect::<String>()),
            Block::ListItem(item) => {
                out.push_str(&item.para.runs.iter().map(|r| r.text.as_str()).collect::<String>())
            }
            Block::Table(table) => {
                for row in &table.rows {
                    for cell in &row.cells {
                        out.push_str(&cell.runs.iter().map(|r| r.text.as_str()).collect::<String>());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn tables(doc: &Doc) -> Vec<&mail_format::Table> {
    doc.blocks
        .iter()
        .filter_map(|b| match b {
            Block::Table(t) => Some(t),
            _ => None,
        })
        .collect()
}

fn columns(table: &mail_format::Table) -> usize {
    table
        .rows
        .iter()
        .map(|row| row.cells.iter().map(|cell| cell.span).sum::<usize>())
        .max()
        .unwrap_or(0)
}

/// 每封邮件都必须解析出内容：文字没被吃掉、没有异常宽表、没有单行表格。
#[test]
fn every_fixture_parses_into_sane_blocks() {
    let entries = manifest();
    assert!(entries.len() >= 100, "金标准语料至少 100 封，实际 {}", entries.len());
    let mut problems = Vec::new();
    for entry in &entries {
        let id = entry["id"].as_str().unwrap();
        let doc = doc_for(id);
        let text = text_of(&doc);
        if text.chars().count() < 12 {
            problems.push(format!("{id}: 正文文字过少（{} 字）", text.chars().count()));
        }
        for table in tables(&doc) {
            let cols = columns(table);
            // 营销邮件的布局表曾把整封邮件撑成十几列窄格，这里守住「不许出现宽表」。
            if cols > 6 {
                problems.push(format!("{id}: 表格 {cols} 列，疑似布局表被当成数据表"));
            }
            if table.rows.len() <= 1 {
                problems.push(format!("{id}: 出现单行表格，说明包裹用的布局表没被摊平"));
            }
        }
    }
    assert!(problems.is_empty(), "共 {} 条异常：\n{}", problems.len(), problems.join("\n"));
}

/// 远程图判定必须和语料特征一致：有远程图的要能认出来，纯链接邮件不能误报。
#[test]
fn remote_image_detection_matches_manifest() {
    let entries = manifest();
    let mut problems = Vec::new();
    for entry in &entries {
        let id = entry["id"].as_str().unwrap();
        let expected = entry["remote_images"].as_u64().unwrap_or(0) > 0;
        let doc = doc_for(id);
        let actual = mail_format::has_remote_images(&doc);
        if expected != actual {
            problems.push(format!("{id}: 期望 remote={expected}，实际 {actual}"));
        }
    }
    assert!(problems.is_empty(), "远程图判定不一致：\n{}", problems.join("\n"));
}

/// 居中单卡形态的按钮必须能被识别成「独立短链接」，否则 CTA 会退化成普通正文。
#[test]
fn cta_shapes_expose_a_standalone_link() {
    let entries = manifest();
    let mut checked = 0;
    for entry in &entries {
        if entry["cta"].as_bool() != Some(true) {
            continue;
        }
        let id = entry["id"].as_str().unwrap();
        let doc = doc_for(id);
        let standalone = doc.blocks.iter().any(|block| match block {
            Block::Paragraph(p) => {
                let runs: Vec<_> = p.runs.iter().filter(|r| !r.text.trim().is_empty()).collect();
                runs.len() == 1 && runs[0].link.is_some()
            }
            _ => false,
        });
        assert!(standalone, "{id}: 没有识别出独立链接按钮");
        checked += 1;
    }
    assert!(checked >= 20, "CTA 形态样本太少：{checked}");
}

/// 单封调试：`DAOZAI_DUMP=b-01 cargo test --test mail_fixtures dump_one -- --ignored --nocapture`
#[test]
#[ignore]
fn dump_one() {
    let id = std::env::var("DAOZAI_DUMP").unwrap_or_else(|_| "b-01".to_string());
    let doc = doc_for(&id);
    println!("== {id} confidence={} ==", doc.confidence);
    for (index, block) in doc.blocks.iter().enumerate() {
        match block {
            Block::Paragraph(p) => {
                let text: String = p.runs.iter().map(|r| r.text.as_str()).collect();
                let links: Vec<_> = p.runs.iter().filter_map(|r| r.link.as_deref()).collect();
                println!(
                    "[{index}] P align={:?} bg={:?} runs={} links={:?} text={:?}",
                    p.align,
                    p.bg,
                    p.runs.len(),
                    links,
                    text.chars().take(80).collect::<String>()
                );
            }
            Block::Image(image) => {
                println!("[{index}] IMG {} {}x{:?}", image.src, image.width.unwrap_or(0.0), image.height)
            }
            Block::Table(table) => {
                println!(
                    "[{index}] TABLE rows={} cols={} bordered={}",
                    table.rows.len(),
                    columns(table),
                    table.bordered
                );
                for row in &table.rows {
                    let cells: Vec<String> = row
                        .cells
                        .iter()
                        .map(|cell| {
                            let text: String = cell.runs.iter().map(|r| r.text.as_str()).collect();
                            format!("{}({}图)", text.chars().take(24).collect::<String>(), cell.images.len())
                        })
                        .collect();
                    println!("      | {}", cells.join(" ‖ "));
                }
            }
            other => println!("[{index}] {other:?}"),
        }
    }
}

/// 调试真实邮件 HTML：`DAOZAI_DUMP_FILE=.tmpwork/real-github.html cargo test --test mail_fixtures dump_file -- --ignored --nocapture`
#[test]
#[ignore]
fn dump_file() {
    let path = std::env::var("DAOZAI_DUMP_FILE").expect("需要 DAOZAI_DUMP_FILE");
    let html = std::fs::read_to_string(&path).expect("读不到文件");
    let doc = mail_format::render(&html, "");
    let images: Vec<_> = doc
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Image(i) => Some(i.src.clone()),
            _ => None,
        })
        .collect();
    println!("== {path} 图 {} 张 / 表 {} 个 / 置信度 {} ==", images.len(), tables(&doc).len(), doc.confidence);
    for (index, src) in images.iter().enumerate() {
        println!("  [{index}] {src}");
    }
    println!("--- 正文 ---");
    for (index, block) in doc.blocks.iter().enumerate() {
        match block {
            Block::Paragraph(p) => {
                let text: String = p.runs.iter().map(|r| r.text.as_str()).collect();
                println!("[{index}] P {:?}", text.chars().take(100).collect::<String>());
            }
            Block::Table(t) => {
                println!("[{index}] TABLE {}x{}", t.rows.len(), columns(t));
                for row in &t.rows {
                    let cells: Vec<String> = row
                        .cells
                        .iter()
                        .map(|cell| {
                            let text: String = cell.runs.iter().map(|r| r.text.as_str()).collect();
                            format!("{}({}图)", text.chars().take(28).collect::<String>(), cell.images.len())
                        })
                        .collect();
                    println!("      | {}", cells.join(" ‖ "));
                }
            }
            Block::Image(image) => println!("[{index}] IMG {} {:?}x{:?}", image.src, image.width, image.height),
            other => println!("[{index}] {other:?}"),
        }
    }
}

/// 分布报告：`cargo test --test mail_fixtures -- --ignored --nocapture`
#[test]
#[ignore]
fn corpus_report() {
    let entries = manifest();
    let mut per_shape: BTreeMap<String, (usize, u32, u32, u32, usize)> = BTreeMap::new();
    for entry in &entries {
        let id = entry["id"].as_str().unwrap();
        let shape = entry["shape"].as_str().unwrap().to_string();
        let doc = doc_for(id);
        let tables_count = tables(&doc).len() as u32;
        let images = doc
            .blocks
            .iter()
            .filter(|b| matches!(b, Block::Image(_)))
            .count() as u32;
        let stat = per_shape.entry(shape).or_insert((0, 100, 0, 0, 0));
        stat.0 += 1;
        stat.1 = stat.1.min(doc.confidence as u32);
        stat.2 = stat.2.max(doc.confidence as u32);
        stat.3 += tables_count;
        stat.4 = stat.4.max(images as usize);
    }
    println!("形态  封数  置信度min  置信度max  表格总数  单封最多图");
    for (shape, (count, min, max, tables_sum, images)) in per_shape {
        println!("{shape:>4}  {count:>4}  {min:>8}  {max:>8}  {tables_sum:>8}  {images:>10}");
    }
}
