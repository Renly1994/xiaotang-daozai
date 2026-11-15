#!/usr/bin/env python3
"""把 .eml 金标准渲染成浏览器截图基线（可复现、无人值守）。

这份基线是「发件人想要的正确样子」：直接用 Chromium 渲染原始 HTML，
不经过岛仔的清洗/包裹层。用它和岛仔完整视图的截图对照，才能判断
「变好还是变坏」，而不是凭肉眼印象。

用法：
    python render_baseline.py [--limit N] [--force]
输出：baseline/<id>.png + baseline/index.json + assets/*.png
依赖：系统 Edge/Chrome（--headless=new），不需要联网。

远程图（example.test 不可达）会重写成本地生成的等尺寸素材，
这样「图片有没有显示、有没有撑坏布局」在基线里是可见的。
"""
import argparse
import json
import re
import struct
import subprocess
import sys
import zlib
from email import policy
from email.parser import BytesParser
from pathlib import Path

ROOT = Path(__file__).resolve().parent
EML = ROOT / "eml"
BASE = ROOT / "baseline"
TMP = ROOT / ".render-tmp"
ASSETS = TMP / "assets"
EDGE_CANDIDATES = [
    Path(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"),
    Path(r"C:\Program Files\Microsoft\Edge\Application\msedge.exe"),
    Path(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
]


def png(width: int, height: int, rgb: tuple[int, int, int]) -> bytes:
    """生成纯色 PNG（不依赖 Pillow）。"""
    raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))

    def chunk(tag: bytes, data: bytes) -> bytes:
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def ensure_assets() -> None:
    ASSETS.mkdir(parents=True, exist_ok=True)
    wanted = {
        "hero.png": (1200, 400, (28, 42, 74)),
        "logo.png": (96, 24, (40, 120, 200)),
        "banner.png": (760, 180, (16, 20, 24)),
        "spacer.png": (20, 10, (232, 234, 238)),
        "pixel.png": (1, 1, (255, 255, 255)),
    }
    for name, (w, h, rgb) in wanted.items():
        path = ASSETS / name
        if not path.exists():
            path.write_bytes(png(w, h, rgb))


def asset_for(url: str) -> str:
    """把远程 URL 映射到等尺寸本地素材，保留「这里本该有一张图」的信息。"""
    lowered = url.lower()
    if "pixel" in lowered or "track" in lowered:
        return (ASSETS / "pixel.png").resolve().as_uri()
    if "logo" in lowered:
        return (ASSETS / "logo.png").resolve().as_uri()
    if "banner" in lowered:
        return (ASSETS / "banner.png").resolve().as_uri()
    if "hero" in lowered:
        return (ASSETS / "hero.png").resolve().as_uri()
    return (ASSETS / "hero.png").resolve().as_uri()


def html_of(path: Path) -> str:
    msg = BytesParser(policy=policy.default).parsebytes(path.read_bytes())
    parts = msg.walk() if msg.is_multipart() else [msg]
    for part in parts:
        if part.get_content_type() == "text/html":
            return part.get_content()
    for part in parts:
        if part.get_content_type() == "text/plain":
            return "<pre>" + part.get_content() + "</pre>"
    return "<p>(empty)</p>"


CID = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg=="


def localize(html: str) -> str:
    html = re.sub(r'(?i)src=(["\'])(https?:)?//[^"\']*(["\'])',
                  lambda m: f'src="{asset_for(m.group(0))}"', html)
    html = re.sub(r'(?i)src=(["\'])cid:[^"\']*(["\'])', f'src="{CID}"', html)
    html = re.sub(r'(?i)url\(\s*["\']?(https?:)?//[^)]*\)',
                  lambda m: "url(" + asset_for(m.group(0)) + ")", html)
    return html


def edge() -> str:
    for candidate in EDGE_CANDIDATES:
        if candidate.is_file():
            return str(candidate)
    raise SystemExit("未找到 Edge/Chrome，无法生成截图基线")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--force", action="store_true")
    args = ap.parse_args()
    BASE.mkdir(exist_ok=True)
    TMP.mkdir(exist_ok=True)
    ensure_assets()
    exe = edge()
    manifest = json.loads((ROOT / "manifest.json").read_text(encoding="utf-8"))
    if args.limit:
        manifest = manifest[: args.limit]
    index = []
    failed = []
    for n, entry in enumerate(manifest, 1):
        mid = entry["id"]
        png = BASE / f"{mid}.png"
        if png.exists() and not args.force:
            index.append(dict(id=mid, png=png.name, shape=entry["shape"], sha256=entry["sha256"]))
            continue
        html = localize(html_of(EML / f"{mid}.eml"))
        page = TMP / f"{mid}.html"
        page.write_text(
            "<!DOCTYPE html><html><head><meta charset='utf-8'>"
            "<style>html,body{margin:0;background:#ffffff}</style></head><body>" + html + "</body></html>",
            encoding="utf-8",
        )
        out = subprocess.run(
            [exe, "--headless=new", "--disable-gpu", "--no-first-run", "--hide-scrollbars",
             "--window-size=1000,1200", f"--user-data-dir={TMP / 'profile'}",
             f"--screenshot={png}", page.resolve().as_uri()],
            capture_output=True, timeout=90,
        )
        if not png.exists():
            failed.append(mid)
            print(f"[{n}/{len(manifest)}] {mid} FAILED: {out.stderr.decode(errors='replace')[-160:]}",
                  file=sys.stderr)
            continue
        index.append(dict(id=mid, png=png.name, shape=entry["shape"], sha256=entry["sha256"]))
        if n % 20 == 0:
            print(f"[{n}/{len(manifest)}] rendered", flush=True)
    (BASE / "index.json").write_text(
        json.dumps(dict(total=len(index), failed=failed, items=index), ensure_ascii=False, indent=2),
        encoding="utf-8")
    print(f"baseline ready: {len(index)} PNG in {BASE}" + (f", {len(failed)} failed" if failed else ""))


if __name__ == "__main__":
    main()
