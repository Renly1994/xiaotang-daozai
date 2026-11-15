#!/usr/bin/env python3
"""生成 100 封脱敏合成 .eml 金标准（A~E 五种形态 × 20 个变体）。

设计约束：
- 不含任何真实邮箱地址、真实邮件原文或个人信息，域名一律 example.test / example.com。
- 每封邮件都是结构特征不同的独立用例，manifest.json 记录特征与期望断言。
- 判定规则只依赖结构特征，禁止按域名/文件名/发件人特判。
"""
import base64
import hashlib
import json
import quopri
from email.utils import formatdate
from pathlib import Path

ROOT = Path(__file__).resolve().parent
EML = ROOT / "eml"
OUT = ROOT / "manifest.json"

SHAPES = {
    "A": "通栏轻 HTML / 纯文本",
    "B": "居中单卡 + 实心 CTA",
    "C": "深色 hero 营销",
    "D": "中文事务 / logo + 明细",
    "E": "混排多栏 / 数据表",
}

# 20 组变体：宽度、配色、编码、图片、CTA、表格结构都不同。
VARIANTS = [
    dict(w=600, accent="#1f883d", panel="#ffffff", outer="#f2f5fa", enc="qp", remote=False, cid=False, cols=1, cta="立即验证"),
    dict(w=640, accent="#2f6fed", panel="#ffffff", outer="#f6f7f9", enc="b64", remote=True,  cid=False, cols=1, cta="确认邮箱"),
    dict(w=580, accent="#d1242f", panel="#fffdf8", outer="#f0efe9", enc="8bit", remote=False, cid=True,  cols=1, cta="查看详情"),
    dict(w=680, accent="#76b900", panel="#ffffff", outer="#eceff3", enc="qp", remote=True,  cid=True,  cols=2, cta="继续"),
    dict(w=600, accent="#0b6bcb", panel="#ffffff", outer="#fafafa", enc="qp", remote=False, cid=False, cols=1, cta="前往控制台"),
    dict(w=720, accent="#6f42c1", panel="#fdfcff", outer="#f3f0fa", enc="b64", remote=True,  cid=False, cols=2, cta="查看报告"),
    dict(w=560, accent="#e36209", panel="#ffffff", outer="#fff7f0", enc="8bit", remote=False, cid=True,  cols=1, cta="重置密码"),
    dict(w=600, accent="#0969da", panel="#ffffff", outer="#f6f8fa", enc="qp", remote=True,  cid=False, cols=3, cta="查看工作流"),
    dict(w=760, accent="#cf222e", panel="#ffffff", outer="#f5f5f5", enc="qp", remote=False, cid=False, cols=1, cta="了解详情"),
    dict(w=620, accent="#1a7f37", panel="#f8fff8", outer="#eef4ee", enc="b64", remote=True,  cid=True,  cols=2, cta="激活账号"),
    dict(w=600, accent="#8250df", panel="#ffffff", outer="#f7f5fb", enc="8bit", remote=False, cid=False, cols=1, cta="打开链接"),
    dict(w=700, accent="#bf8700", panel="#fffef6", outer="#f7f4e8", enc="qp", remote=True,  cid=False, cols=2, cta="立即处理"),
    dict(w=580, accent="#0550ae", panel="#ffffff", outer="#f0f6ff", enc="qp", remote=False, cid=True,  cols=1, cta="验证设备"),
    dict(w=660, accent="#bc4c00", panel="#ffffff", outer="#fdf3ec", enc="b64", remote=True,  cid=False, cols=3, cta="查看订单"),
    dict(w=600, accent="#116329", panel="#ffffff", outer="#eef7ef", enc="8bit", remote=False, cid=False, cols=1, cta="恢复访问"),
    dict(w=740, accent="#0969da", panel="#fbfdff", outer="#eef4fb", enc="qp", remote=True,  cid=True,  cols=2, cta="阅读原文"),
    dict(w=600, accent="#9a6700", panel="#ffffff", outer="#faf8ed", enc="qp", remote=False, cid=False, cols=1, cta="确认订阅"),
    dict(w=640, accent="#cf222e", panel="#ffffff", outer="#fbeef0", enc="b64", remote=True,  cid=False, cols=2, cta="立即更新"),
    dict(w=560, accent="#3b3b3b", panel="#ffffff", outer="#f4f4f4", enc="8bit", remote=False, cid=True,  cols=1, cta="查看明细"),
    dict(w=680, accent="#0f766e", panel="#fbfffe", outer="#eef7f6", enc="qp", remote=True,  cid=False, cols=3, cta="开始使用"),
]

REMOTE = "https://cdn.example.test/assets/hero-{v}.png"
PIXEL = "https://track.example.test/pixel-{v}.gif"
LOGO = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\nIDATx\x9cc\xf8\xcf\xc0\xf0\x1f\x00\x05\x05\x02\x00\xa5\xf6E\x9e\x00\x00\x00\x00IEND\xaeB`\x82"


def img(v, remote):
    return REMOTE.format(v=v) if remote else "cid:logo-{v}".format(v=v)


def a(shape, v, i):
    """通栏轻 HTML：段落、换行、内联链接、pre、列表、引用、编码变体。"""
    links = "".join(
        f'<p>第 {n} 段说明文字，包含一条 <a href="https://example.test/doc/{i}/{n}">内联链接 {n}</a> 用于验证链接命中。</p>'
        for n in range(1, 4 + i % 3)
    )
    extras = [
        "<pre>code line 1\ncode line 2\n  indented</pre>",
        "<blockquote>这是一段被引用的原文，用来检验引用块样式。</blockquote>",
        "<ul><li>项目一</li><li>项目二</li><li>项目三</li></ul>",
        "<p>验证码 <strong>482913</strong> 有效期 10 分钟。</p>",
    ][i % 4]
    return f"""<!DOCTYPE html><html><head><meta charset="utf-8"></head>
<body style="margin:0;padding:16px;color:#24292f;font-family:Arial,sans-serif">
<h2 style="font-size:18px">形态 A · 通栏通知 {i:02d}</h2>
{links}{extras}
<hr><p style="font-size:12px;color:#57606a">本邮件为合成回归语料，不含真实数据。</p>
</body></html>""", dict(
        tables=0, style_blocks=0, inline_styles=4, remote_images=0,
        cid_images=0, cta=False, nested_tables=0, columns=1,
        pre=i % 4 == 0, list=i % 4 == 2, quote=i % 4 == 1,
        dark_hero=False, global_css=False, positioned=False,
    )


def b(shape, v, i):
    """居中单卡 + 实心 CTA：按钮底色来自内联 style 或 table bgcolor。"""
    style_btn = i % 2 == 0
    btn = (
        f'<a href="https://example.test/verify/{i}" style="display:inline-block;background-color:{v["accent"]};'
        f'color:#ffffff;padding:12px 22px;border-radius:6px;text-decoration:none">{v["cta"]}</a>'
        if style_btn else
        f'<table bgcolor="{v["accent"]}" cellpadding="12" style="border-radius:8px"><tr><td>'
        f'<a href="https://example.test/verify/{i}" style="color:#ffffff;text-decoration:none">{v["cta"]}</a>'
        '</td></tr></table>'
    )
    return f"""<!DOCTYPE html><html><head><meta charset="utf-8">
<style>.h1{{font-size:24px;font-weight:700;color:#1f2328}} .fine{{font-size:12px;color:#57606a}}</style>
</head><body style="margin:0;background:{v['outer']};padding:24px">
<table width="{v['w']}" align="center" cellpadding="24" style="background:{v['panel']};border-radius:12px"><tr><td>
<p class="h1">形态 B · 请确认操作 {i:02d}</p>
<p>我们收到了一个需要你确认的请求。点击下方按钮继续，或忽略本邮件。</p>
<p align="center">{btn}</p>
<p class="fine">如果按钮无法点击，请复制此链接：https://example.test/verify/{i}</p>
<hr><p class="fine">© 2026 Example Test · 退订</p>
</td></tr></table></body></html>""", dict(
        tables=1, style_blocks=1, inline_styles=5, remote_images=0,
        cid_images=0, cta=True, nested_tables=0, columns=1,
        pre=False, list=False, quote=False, dark_hero=False,
        global_css=True, positioned=False, cta_inline_style=style_btn,
    )


def c(shape, v, i):
    """深色 hero 营销：外围浅灰、黑底大标题、远程图、spacer、追踪像素、协议相对 URL。"""
    remote = REMOTE.format(v=i)
    proto = "//cdn.example.test/assets/banner-{v}.png".format(v=i)
    pixel = f'<img src="{PIXEL.format(v=i)}" width="1" height="1" alt="">' if v["remote"] else ""
    spacer = f'<img src="{proto}" width="20" height="10" alt="">'
    hero_bg = "background-image:url({r});background-color:#111418;".format(r=remote) if v["remote"] else "background-color:#111418;"
    return f"""<!DOCTYPE html><html><head><meta charset="utf-8">
<style>.hero{{color:#ffffff;font-size:34px;font-weight:700}} .sub{{color:#d0d7de}} .price{{color:#ff6a5e;font-size:22px}}</style>
</head><body style="margin:0;background:{v['outer']};padding:20px">
<table width="{v['w']}" align="center" cellpadding="0" cellspacing="0"><tr><td style="{hero_bg}padding:36px">
<span class="hero">限时活动 {i:02d}</span><p class="sub">多行副标题用于验证深色底上的文字对比度。</p>
<p class="price">¥ {199 + i}.00</p>
<p><a href="https://example.test/offer/{i}" style="background:{v['accent']};color:#fff;padding:12px 26px;border-radius:8px;text-decoration:none">{v['cta']}</a></p>
</td></tr>
<tr><td bgcolor="{v['panel']}" style="padding:20px">{spacer}<p>下方是商品列表与说明文字。</p>{pixel}</td></tr>
</table></body></html>""", dict(
        tables=1, style_blocks=1, inline_styles=6,
        remote_images=(2 if v["remote"] else 0) + 1, cid_images=0,
        cta=True, nested_tables=0, columns=1, pre=False, list=False,
        quote=False, dark_hero=True, global_css=True, positioned=False,
        protocol_relative=True, spacer=True, tracking_pixel=v["remote"],
    )


def d(shape, v, i):
    """中文事务：logo + 订单明细表 + 二维码占位 + 底部单链接。"""
    rows = "".join(
        f"<tr><td style='padding:6px;border-bottom:1px solid #eaeef2'>项目 {n}</td>"
        f"<td align='right' style='padding:6px'>¥ {10 * n + i}.00</td></tr>"
        for n in range(1, 3 + i % 3)
    )
    logo = f'<img src="{img(i, v["remote"])}" width="96" height="24" alt="Logo">'
    return f"""<!DOCTYPE html><html><head><meta charset="utf-8"></head>
<body style="margin:0;padding:24px;background:{v['outer']};font-family:'Microsoft YaHei',sans-serif">
<table width="{v['w']}" align="center" bgcolor="{v['panel']}" cellpadding="24"><tr><td>
{logo}
<h3 style="font-size:17px;color:#1f2328">形态 D · 账户变动提醒 {i:02d}</h3>
<p style="font-size:13px;color:#57606a">你好，以下是本次变动的明细，请核对。</p>
<table width="100%" cellpadding="0" cellspacing="0">{rows}</table>
<p style="font-size:12px;color:#8c959f">如需帮助，请联系支持团队（本邮件为合成语料）。</p>
<p><a href="https://example.test/help/{i}" style="color:{v['accent']}">查看完整明细</a></p>
</td></tr></table></body></html>""", dict(
        tables=2, style_blocks=0, inline_styles=8,
        remote_images=1 if v["remote"] else 0,
        cid_images=0 if v["remote"] else 1, cta=False,
        nested_tables=1, columns=2, pre=False, list=False, quote=False,
        dark_hero=False, global_css=False, positioned=False, data_table=True,
    )


def e(shape, v, i):
    """混排多栏：浅灰外围 + 白卡 + 多栏布局表 / 有边框数据表 / 全局 CSS 选择器。"""
    cols = max(2, v["cols"])
    cells = "".join(
        f"<td width='{100 // cols}%' valign='top' style='padding:12px;background:{v['panel']}'>"
        f"<strong>栏目 {n}</strong><p style='font-size:13px;color:#57606a'>第 {n} 栏的说明文字，用于验证布局表摊平。</p></td>"
        for n in range(1, cols + 1)
    )
    border = 'border="1" cellspacing="0" cellpadding="6"' if i % 2 == 0 else 'cellpadding="6"'
    grid = "".join(
        "<tr>" + "".join(f"<td>数据 {r}-{c}</td>" for c in range(1, 4)) + "</tr>"
        for r in range(1, 3)
    )
    positioned = '<div style="position:absolute;top:0;left:0">角标</div>' if i % 3 == 0 else ""
    return f"""<!DOCTYPE html><html><head><meta charset="utf-8">
<style>body{{margin:0}} h1{{font-size:22px}} a{{color:{v['accent']}}} td{{font-size:13px}}</style>
</head><body style="background:{v['outer']};padding:20px">
<table width="{v['w']}" align="center" cellpadding="16" bgcolor="{v['panel']}"><tr><td>
<h1>形态 E · 混排周报 {i:02d}</h1>{positioned}
<table width="100%" cellpadding="0" cellspacing="0"><tr>{cells}</tr></table>
<table {border} width="100%">{grid}</table>
<p><a href="https://example.test/report/{i}">查看完整报告</a></p>
</td></tr></table></body></html>""", dict(
        tables=3, style_blocks=1, inline_styles=6, remote_images=0,
        cid_images=0, cta=False, nested_tables=2, columns=cols,
        pre=False, list=False, quote=False, dark_hero=False,
        global_css=True, positioned=i % 3 == 0,
        data_table=i % 2 == 0, bordered_table=i % 2 == 0,
    )


BUILDERS = {"A": a, "B": b, "C": c, "D": d, "E": e}


def encode_body(html, enc):
    if enc == "qp":
        return "quoted-printable", quopri.encodestring(html.encode("utf-8")).decode("ascii")
    if enc == "b64":
        return "base64", base64.encodebytes(html.encode("utf-8")).decode("ascii")
    return "8bit", html


def build_eml(mid, shape, i, v, html, features, cid):
    cte, body = encode_body(html, v["enc"])
    subject = f"[{shape}-{i:02d}] {SHAPES[shape]} · 合成回归语料 {mid}"
    headers = [
        "From: Example Test <notice@example.test>",
        "To: Recipient <reader@example.test>",
        f"Subject: {subject}",
        f"Date: {formatdate(1762000000 + i * 3600, usegmt=True)}",
        f"Message-ID: <{mid}@example.test>",
        "MIME-Version: 1.0",
    ]
    if cid and not v["remote"]:
        bound, bound2 = "fixture-related", "fixture-alt"
        parts = [
            f"Content-Type: multipart/related; boundary=\"{bound}\"",
            "",
            f"--{bound}",
            f"Content-Type: multipart/alternative; boundary=\"{bound2}\"",
            "",
            f"--{bound2}",
            "Content-Type: text/plain; charset=utf-8",
            "Content-Transfer-Encoding: 8bit",
            "",
            "这是纯文本备选部分。",
            f"--{bound2}",
            "Content-Type: text/html; charset=utf-8",
            f"Content-Transfer-Encoding: {cte}",
            "",
            body,
            f"--{bound2}--",
            f"--{bound}",
            "Content-Type: image/png",
            "Content-Transfer-Encoding: base64",
            "Content-ID: <logo-{v}>".format(v=i),
            "Content-Disposition: inline",
            "",
            base64.encodebytes(LOGO).decode("ascii"),
            f"--{bound}--",
            "",
        ]
        return "\r\n".join(headers + parts).replace("\r\n\r\n", "\r\n\r\n", 1), features
    headers += [
        "Content-Type: text/html; charset=utf-8",
        f"Content-Transfer-Encoding: {cte}",
        "",
        body,
        "",
    ]
    return "\r\n".join(headers), features


def main():
    EML.mkdir(parents=True, exist_ok=True)
    manifest = []
    for shape, label in SHAPES.items():
        for i, v in enumerate(VARIANTS):
            mid = f"{shape.lower()}-{i:02d}"
            html, features = BUILDERS[shape](shape, v, i)
            text, features = build_eml(mid, shape, i, v, html, features, cid=v["cid"])
            (EML / f"{mid}.eml").write_bytes(text.encode("utf-8"))
            features = dict(features)
            features.update(
                id=mid,
                shape=shape,
                shape_label=label,
                variant=i,
                encoding=v["enc"],
                width=v["w"],
                multipart=bool(v["cid"] and not v["remote"]),
                sha256=hashlib.sha256(html.encode("utf-8")).hexdigest(),
            )
            manifest.append(features)
    OUT.write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"generated {len(manifest)} .eml files in {EML}")
    for shape in SHAPES:
        rows = [m for m in manifest if m["shape"] == shape]
        print(f"  {shape}: {len(rows)} 封, 远程图 {sum(m['remote_images'] > 0 for m in rows)} 封, "
              f"CTA {sum(m['cta'] for m in rows)} 封, 表格 {sum(m['tables'] for m in rows)} 个")


if __name__ == "__main__":
    main()
