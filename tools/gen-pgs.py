#!/usr/bin/env python3
"""Write a small PGS (HDMV presentation graphic stream) subtitle file (.sup) for the subtitle tests.

ffmpeg has a PGS decoder but no encoder, so this builds the segments by hand: a 320x240 stream with two bitmaps
(a white bar with a red stripe at 1.0 s, a green box at 4.0 s that is sent in two object fragments), each cleared
1.5 s later. Usage: gen-pgs.py out.sup

Segment layout (public Blu-ray PGS description): "PG", PTS and DTS (90 kHz, 32 bit), a type byte, a 16-bit size,
then the payload; types 0x16 composition, 0x17 window, 0x14 palette, 0x15 object, 0x80 end.
"""
import struct
import sys

W, H = 320, 240


def segment(pts_s, kind, body):
    pts = int(round(pts_s * 90000))
    return b"PG" + struct.pack(">II", pts, 0) + bytes([kind]) + struct.pack(">H", len(body)) + body


def rle_row(row):
    """Run-length code one row of palette indices (a PGS object row ends with 00 00)."""
    out = bytearray()
    i = 0
    while i < len(row):
        j = i
        while j < len(row) and row[j] == row[i] and j - i < 0x3FFF:
            j += 1
        run, col = j - i, row[i]
        if col == 0:
            if run < 64:
                out += bytes([0, run])
            else:
                out += bytes([0, 0x40 | (run >> 8), run & 255])
        elif run < 3:
            out += bytes([col]) * run
        elif run < 64:
            out += bytes([0, 0x80 | run, col])
        else:
            out += bytes([0, 0xC0 | (run >> 8), run & 255, col])
        i = j
    out += bytes([0, 0])
    return bytes(out)


def bitmap(w, h, painter):
    rows = [[painter(x, y) for x in range(w)] for y in range(h)]
    return b"".join(rle_row(r) for r in rows)


def ycc(r, g, b):
    """Limited-range BT.709 YCbCr of an 8-bit RGB colour, returned as (Y, Cr, Cb) as PGS stores them."""
    y = 16 + 0.1826 * r + 0.6142 * g + 0.0620 * b
    cb = 128 - 0.1006 * r - 0.3386 * g + 0.4392 * b
    cr = 128 + 0.4392 * r - 0.3989 * g - 0.0403 * b
    return tuple(max(0, min(255, int(round(v)))) for v in (y, cr, cb))


def palette(pid, entries):
    body = bytes([pid, 0])
    for idx, (r, g, b, a) in entries.items():
        y, cr, cb = ycc(r, g, b)
        body += bytes([idx, y, cr, cb, a])
    return body


def object_segments(pts, oid, w, h, data, fragments=1):
    """The ODS segments of one object, in `fragments` pieces."""
    chunks = [data[i * len(data) // fragments:(i + 1) * len(data) // fragments] for i in range(fragments)]
    out = b""
    for n, c in enumerate(chunks):
        flag = (0x80 if n == 0 else 0) | (0x40 if n == fragments - 1 else 0)
        body = struct.pack(">HBB", oid, 0, flag)
        if n == 0:
            body += struct.pack(">I", len(data) + 4)[1:] + struct.pack(">HH", w, h)
        out += segment(pts, 0x15, body + c)
    return out


def pcs(pts, num, state, pid, comps):
    body = struct.pack(">HHBHBBBB", W, H, 0x10, num, state, 0, pid, len(comps))
    for oid, x, y in comps:
        body += struct.pack(">HBBHH", oid, 0, 0, x, y)
    return segment(pts, 0x16, body)


def wds(pts, wid, x, y, w, h):
    return segment(pts, 0x17, bytes([1]) + struct.pack(">BHHHH", wid, x, y, w, h))


def display_set(pts, num, state, pid, comps, window=None, pal=None, objs=b""):
    out = pcs(pts, num, state, pid, comps)
    if window:
        out += wds(pts, 0, *window)
    if pal:
        out += segment(pts, 0x14, pal)
    out += objs
    out += segment(pts, 0x80, b"")
    return out


def main(path):
    # A first (empty) display set at 0, so the stream starts at zero: ffmpeg shifts timestamps by the start of its input.
    sup = display_set(0.0, 0, 0x80, 0, [])
    # Bitmap 1: 120x28, a white bar (index 1) with a red stripe (index 2) across the middle and transparent corners (0).
    w1, h1 = 120, 28

    def p1(x, y):
        if (x < 3 or x >= w1 - 3) and (y < 3 or y >= h1 - 3):
            return 0
        return 2 if 12 <= y < 16 else 1

    pal1 = palette(0, {0: (0, 0, 0, 0), 1: (255, 255, 255, 255), 2: (200, 30, 30, 255)})
    d1 = bitmap(w1, h1, p1)
    sup += display_set(1.0, 1, 0x80, 0, [(1, 100, 180)], (100, 180, w1, h1), pal1, object_segments(1.0, 1, w1, h1, d1))
    sup += display_set(2.5, 2, 0x00, 0, [], (100, 180, w1, h1))
    # Bitmap 2: 64x40, a green box with a yellow dot, in two fragments, at the top right.
    w2, h2 = 64, 40

    def p2(x, y):
        return 2 if (x - 32) ** 2 + (y - 20) ** 2 < 36 else 1

    pal2 = palette(0, {0: (0, 0, 0, 0), 1: (40, 180, 60, 255), 2: (250, 230, 40, 255)})
    d2 = bitmap(w2, h2, p2)
    sup += display_set(4.0, 3, 0x80, 0, [(1, 230, 20)], (230, 20, w2, h2), pal2, object_segments(4.0, 1, w2, h2, d2, 2))
    sup += display_set(5.5, 4, 0x00, 0, [], (230, 20, w2, h2))
    with open(path, "wb") as f:
        f.write(sup)


if __name__ == "__main__":
    main(sys.argv[1])
