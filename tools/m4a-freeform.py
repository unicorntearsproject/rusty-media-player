#!/usr/bin/env python3
"""Add iTunes-style free-form items (`----`, owner com.apple.iTunes) and the `pgap` gapless-album flag to an M4A.

    m4a-freeform.py in.m4a out.m4a name=value [name=value ...] [--pgap]

ffmpeg writes only the standard items (and, with `-movflags use_metadata_tags`, QuickTime `mdta` keys), but ReplayGain in files from iTunes,
foobar2000 and MP3Tag sits in `----` items, so the test fixtures need them put in by hand. The file must have `moov` after `mdat`
(ffmpeg's default), so growing `moov` moves no sample offsets, and must already have `moov/udta/meta/ilst` (give it a title tag).
"""
import struct
import sys


def box(ty: bytes, body: bytes) -> bytes:
    return struct.pack(">I4s", len(body) + 8, ty) + body


def find(data: bytes, start: int, end: int, ty: bytes, skip: int = 0):
    """(offset, size) of the first child box `ty` in data[start:end]."""
    off = start
    while off + 8 <= end:
        size, t = struct.unpack(">I4s", data[off : off + 8])
        if size < 8:
            break
        if t == ty:
            return off, size
        off += size
    raise SystemExit(f"no {ty!r} box")


def main() -> None:
    args = sys.argv[1:]
    src, dst, rest = args[0], args[1], args[2:]
    data = bytearray(open(src, "rb").read())
    items = b""
    for a in rest:
        if a == "--pgap":
            items += box(b"pgap", box(b"data", struct.pack(">II", 21, 0) + b"\x01"))
            continue
        name, value = a.split("=", 1)
        items += box(
            b"----",
            box(b"mean", b"\0\0\0\0com.apple.iTunes")
            + box(b"name", b"\0\0\0\0" + name.encode())
            + box(b"data", struct.pack(">II", 1, 0) + value.encode()),
        )
    # moov -> udta -> meta (a full box: 4 bytes of version and flags) -> ilst
    moov = find(data, 0, len(data), b"moov")
    udta = find(data, moov[0] + 8, moov[0] + moov[1], b"udta")
    meta = find(data, udta[0] + 8, udta[0] + udta[1], b"meta")
    ilst = find(data, meta[0] + 12, meta[0] + meta[1], b"ilst")
    end_of_ilst = ilst[0] + ilst[1]
    data[end_of_ilst:end_of_ilst] = items
    for off, size in (ilst, meta, udta, moov):
        struct.pack_into(">I", data, off, size + len(items))
    open(dst, "wb").write(bytes(data))


main()
