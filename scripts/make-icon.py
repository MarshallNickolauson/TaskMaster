#!/usr/bin/env python3
"""Generates the TaskMaster logo (SVG + PNGs) from one shared geometry. Pure stdlib."""
import math, struct, sys, zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
S = 1024.0
BG = ((22, 31, 48), (9, 13, 22))  # top-left, bottom-right
# (x0, y0, x1, y1, base colour): a load meter climbing from cool to hot
TILES = [
    (148, 560, 298, 876, (34, 211, 238)),
    (340, 420, 490, 876, (45, 212, 160)),
    (532, 284, 682, 876, (251, 191, 36)),
    (724, 148, 874, 876, (244, 63, 94)),
]
GAP_R = 26  # tile corner radius
BG_R = 232  # background corner radius


def light(c, f):
    return tuple(max(0, min(255, round(v * f))) for v in c)


def svg():
    defs, body = [], []
    for i, (x0, y0, x1, y1, c) in enumerate(TILES):
        a, b = light(c, 1.18), light(c, 0.82)
        defs.append(f'<linearGradient id="t{i}" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="rgb{a}"/><stop offset="1" stop-color="rgb{b}"/></linearGradient>')
        body.append(f'<rect x="{x0}" y="{y0}" width="{x1-x0}" height="{y1-y0}" rx="{GAP_R}" fill="url(#t{i})"/>')
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">'
        f'<defs><linearGradient id="bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="rgb{BG[0]}"/><stop offset="1" stop-color="rgb{BG[1]}"/></linearGradient>'
        + "".join(defs) + '</defs>'
        f'<rect width="1024" height="1024" rx="{BG_R}" fill="url(#bg)"/>' + "".join(body) + '</svg>'
    )


def sd_rrect(px, py, x0, y0, x1, y1, r):
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    qx, qy = abs(px - cx) - ((x1 - x0) / 2 - r), abs(py - cy) - ((y1 - y0) / 2 - r)
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - r


def mix(a, b, t):
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(3))


def render(size):
    k = S / size
    rows = []
    for j in range(size):
        row = bytearray()
        for i in range(size):
            px, py = (i + 0.5) * k, (j + 0.5) * k
            d = sd_rrect(px, py, 0, 0, S, S, BG_R)
            cov = max(0.0, min(1.0, 0.5 - d / k))
            if cov <= 0:
                row += b"\0\0\0\0"
                continue
            col = mix(BG[0], BG[1], (px + py) / (2 * S))
            for x0, y0, x1, y1, c in TILES:
                dt = sd_rrect(px, py, x0, y0, x1, y1, GAP_R)
                tc = max(0.0, min(1.0, 0.5 - dt / k))
                if tc > 0:
                    t = ((px - x0) / (x1 - x0) + (py - y0) / (y1 - y0)) / 2
                    col = mix(col, mix(light(c, 1.18), light(c, 0.82), max(0, min(1, t))), tc)
            row += bytes([round(col[0]), round(col[1]), round(col[2]), round(cov * 255)])
        rows.append(bytes(row))
    return rows


def png(size):
    raw = b"".join(b"\0" + r for r in render(size))
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


if __name__ == "__main__":
    (ROOT / "assets").mkdir(exist_ok=True)
    (ROOT / "assets/icon.svg").write_text(svg())
    for n in (int(a) for a in (sys.argv[1:] or ["256", "512"])):
        (ROOT / f"assets/icon-{n}.png").write_bytes(png(n))
        print("wrote", n)
