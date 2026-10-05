#!/usr/bin/env python3
"""Generate the M10 test library with ffmpeg: about 200 tiny tracks across 20 albums in mixed formats (MP3, FLAC,
Opus, Vorbis, AAC in M4A, WAV), with and without embedded art, folder art, Unicode and odd or missing tags.

    tools/gen-library.py <outdir>

Writes <outdir>/music/<Artist>/<Album>/<track files>, plus <outdir>/expected.json: the tree the scanner must find
(derived from the spec below, not from any scan, so it is an independent oracle). Existing output is reused when
the spec has not changed (a stamp file), so calling this from the fixture script is cheap.
"""
import base64
import hashlib
import json
import os
import struct
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

VERSION = "library-v3"

# (album artist as it must be indexed, album, year, genre, formats, tracks, art, extras)
# art: "jpeg" / "png" embedded in every track, "folder:cover.jpg" a file next to the tracks, None.
# extras: discs=N, various="tag" (album artist tag present) or "notag" (compilation without one), tags="none"/"v1"/
#         "noalbum"/"wav", mixed=True (cycle the formats).
ALBUMS = [
    dict(artist="Aurora Vale", album="Polar Nights", year=2019, genre="Ambient", fmts=["mp3"], n=12, art="jpeg"),
    dict(artist="Aurora Vale", album="Daybreak", year=2021, genre="Ambient", fmts=["flac"], n=10, art="png"),
    dict(artist="The Midnight Owls", album="Hoot & Holler", year=2015, genre="Rock", fmts=["opus"], n=8, art="jpeg"),
    dict(artist="The Midnight Owls", album="After Dark", year=2017, genre="Rock", fmts=["ogg"], n=10, art="folder:cover.jpg"),
    dict(artist="Zoë Pérez", album="Corazón de Neón", year=2020, genre="Pop", fmts=["m4a"], n=12, art="jpeg"),
    dict(artist="Ünal Çelik", album="İstanbul Gecesi", year=2012, genre="World", fmts=["mp3"], n=9, art="png"),
    dict(artist="Bjørn Åkesson", album="Fjord Echoes", year=2018, genre="Folk", fmts=["flac"], n=11, art="jpeg", discs=2),
    dict(artist="東京ネオン", album="夜のドライブ", year=2022, genre="Synthwave", fmts=["mp3"], n=10, art="jpeg"),
    dict(artist="Кино-Лес", album="Тишина", year=2008, genre="Post-rock", fmts=["flac"], n=8, art="folder:folder.png"),
    dict(artist="Various Artists", album="Neon Mixtape Vol. 1", year=2023, genre="Electronic", fmts=["mp3", "flac", "opus", "m4a", "ogg"], n=16, art="jpeg", various="tag"),
    dict(artist="Various Artists", album="Road Trip Rips", year=2010, genre="Rock", fmts=["mp3"], n=10, art=None, various="notag"),
    dict(artist="Unknown Artist", album="Unknown Album", year=None, genre=None, fmts=["mp3"], n=6, art=None, tags="none", dir="Untagged Rips"),
    dict(artist="Sigurður Þórsson", album="Ísland", year=2016, genre="Classical", fmts=["wav"], n=8, art=None, tags="wav"),
    dict(artist="Old Tapes", album="Side A", year=1999, genre="Rock", fmts=["mp3"], n=6, art=None, tags="v1"),
    dict(artist="Quiet Machines", album="Signal & Noise", year=2014, genre="IDM", fmts=["m4a"], n=14, art="jpeg"),
    dict(artist="Delta Hiss", album="Unknown Album", year=None, genre="Noise", fmts=["mp3"], n=5, art=None, tags="noalbum"),
    dict(artist="Delta Hiss", album="Feedback Loop", year=2020, genre="Noise", fmts=["flac"], n=12, art=None),
    dict(artist="Mono Mimi", album="Lofi Sketches", year=2024, genre="Lo-fi", fmts=["ogg"], n=11, art="png"),
    dict(artist="A Tribe of Pines", album="Evergreen", year=2013, genre="Folk", fmts=["opus"], n=9, art="folder:Folder.JPG"),
    dict(artist="DJ Unicorn Tears", album='Tears for Fears (Remixes) [Deluxe]', year=2025, genre="Electronic", fmts=["mp3"], n=14, art="jpeg"),
]
assert sum(a["n"] for a in ALBUMS) == 201

WORDS = ["Glass", "Ember", "Static", "Harbor", "Lantern", "Velvet", "Orbit", "Pulse", "Mirage", "Echo", "Drift", "Neon",
         "Meadow", "Circuit", "Whisper", "Tidal", "Prism", "Cinder", "Aurora", "Signal", "Hollow", "Rain", "Violet", "Wire"]
UNI = ["Café", "Señor", "Über", "Naïve", "Crème", "Façade", "Ångström", "Déjà"]
GUEST = ["Nova Kane", "Lio", "The Static Kids", "Mélanie Roux", "Okonkwo", "Pixel Sage", "Hanna Wülf", "Bluebird & Co"]


def track_title(ai, i):
    w = WORDS[(ai * 7 + i * 5) % len(WORDS)]
    w2 = WORDS[(ai * 3 + i * 11 + 4) % len(WORDS)]
    if (ai + i) % 6 == 0:
        return f"{UNI[(ai + i) % len(UNI)]} {w}"
    return f"{w} {w2}"


def safe(s):
    return "".join("_" if c in '/\\:*?"<>|' else c for c in s)


def hue_rgb(ai):
    # A distinct, fairly saturated colour per album (not too dark, so JPEG error stays small).
    import colorsys
    r, g, b = colorsys.hsv_to_rgb((ai * 0.173) % 1.0, 0.75, 0.9)
    return [int(r * 255), int(g * 255), int(b * 255)]


def run(cmd):
    r = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if r.returncode != 0:
        raise SystemExit(f"command failed: {' '.join(cmd[:6])}...\n{r.stderr}")


def make_cover(path, rgb, kind, size):
    hexc = "0x%02x%02x%02x" % tuple(rgb)
    fmt = "yuvj420p" if kind == "jpeg" else "rgb24"
    run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", f"color=c={hexc}:s={size}x{size}:d=1",
         "-vf", f"format={fmt}", "-frames:v", "1", *(["-q:v", "2"] if kind == "jpeg" else []), path])


def flac_picture_block(data, mime):
    def lp(b):
        return struct.pack(">I", len(b)) + b
    return struct.pack(">I", 3) + lp(mime.encode()) + lp(b"") + struct.pack(">IIII", 0, 0, 0, 0) + lp(data)


def main():
    out = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/fixtures/library")
    stamp = os.path.join(out, ".done")
    me = hashlib.sha1(open(__file__, "rb").read()).hexdigest()[:12]
    if os.path.exists(stamp) and open(stamp).read().strip() == f"{VERSION}-{me}":
        return
    music = os.path.join(out, "music")
    # Overwrite in place (no recursive deletes): a changed spec reuses the same paths; a stale extra file would make
    # the track-count check in the tests fail loudly, which is the cue to start from a fresh directory.
    os.makedirs(music, exist_ok=True)
    jobs, expected = [], []
    covers = os.path.join(out, ".covers")
    os.makedirs(covers, exist_ok=True)
    for ai, a in enumerate(ALBUMS):
        rgb = hue_rgb(ai)
        dirname = a.get("dir") or os.path.join(safe(a["artist"]), safe(a["album"]))
        d = os.path.join(music, dirname)
        os.makedirs(d, exist_ok=True)
        art = a["art"]
        art_file = None
        art_kind = None
        if art in ("jpeg", "png"):
            art_file = os.path.join(covers, f"{ai}.{'jpg' if art == 'jpeg' else 'png'}")
            make_cover(art_file, rgb, art, 300)
            art_kind = "embedded"
        elif art and art.startswith("folder:"):
            name = art.split(":", 1)[1]
            make_cover(os.path.join(d, name), rgb, "png" if name.lower().endswith("png") else "jpeg", 240)
            art_kind = "folder"
        entry = dict(artist=a["artist"], album=a["album"], year=a["year"], genre=a["genre"], art=art_kind,
                     art_rgb=rgb if art_kind else None, dir=dirname.replace(os.sep, "/"), tracks=[])
        n, discs = a["n"], a.get("discs", 1)
        per_disc = (n + discs - 1) // discs
        tags_mode = a.get("tags", "full")
        for i in range(n):
            fmt = a["fmts"][i % len(a["fmts"])]
            disc = i // per_disc + 1
            tno = i % per_disc + 1
            title = track_title(ai, i)
            if a.get("various"):
                artist = GUEST[(ai + i) % len(GUEST)]
            else:
                artist = a["artist"]
            stem = f"{tno:02d} {safe(title)}"
            if discs > 1:
                stem = f"{disc}-{stem}"
            fname = f"{stem}.{fmt}"
            if tags_mode == "none":
                fname = f"{tno:02d} - {safe(title)}.{fmt}"
            path = os.path.join(d, fname)
            dur = 0.4 + (i % 3) * 0.1
            freq = 220 + 37 * ((ai * 5 + i) % 24)
            tags = {}
            if tags_mode == "full":
                tags = dict(title=title, artist=artist, album=a["album"], track=f"{tno}/{per_disc}", genre=a["genre"] or "",
                            date=str(a["year"]) if a["year"] else "")
                if a.get("various") == "tag":
                    tags["album_artist"] = "Various Artists"
                elif not a.get("various"):
                    tags["album_artist"] = a["artist"] if ai % 2 == 0 else ""
                if discs > 1:
                    tags["disc"] = f"{disc}/{discs}"
            elif tags_mode == "noalbum":
                tags = dict(title=title, artist=artist, track=str(tno))
            elif tags_mode == "v1":
                tags = dict(title=title, artist=artist, album=a["album"], track=str(tno), date=str(a["year"]))
            elif tags_mode == "wav":
                tags = dict(title=title, artist=artist, album=a["album"], track=str(tno), date=str(a["year"]), genre=a["genre"])
            jobs.append(dict(path=path, fmt=fmt, dur=dur, freq=freq, tags=tags, art_file=art_file, art=art, v1=tags_mode == "v1",
                             none=tags_mode == "none"))
            exp_title = title if tags_mode not in ("none",) else f"{tno:02d} - {safe(title)}"
            exp_artist = artist if tags_mode not in ("none",) else "Unknown Artist"
            exp = dict(path=f"{entry['dir']}/{fname}", title=exp_title, artist=exp_artist, track=tno if tags_mode != "none" else None,
                       disc=disc if discs > 1 else None, duration_s=dur, format=fmt)
            if tags_mode == "none":
                exp["track"] = None
            entry["tracks"].append(exp)
        if a.get("various") == "notag":
            entry["artist"] = "Various Artists"
        expected.append(entry)
        # A cover in a folder that also has tracks (not named like cover art) must be ignored.
        if ai == 0:
            with open(os.path.join(d, "notes.txt"), "w", encoding="utf-8") as f:
                f.write("not a track\n")

    def render(j):
        fmt = j["fmt"]
        base = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i",
                f"sine=frequency={j['freq']}:sample_rate=24000:duration={j['dur']}"]
        codec = dict(mp3=["-c:a", "libmp3lame", "-b:a", "48k"], flac=["-c:a", "flac"], opus=["-c:a", "libopus", "-b:a", "24k"],
                     ogg=["-c:a", "libvorbis", "-q:a", "2"], m4a=["-c:a", "aac", "-b:a", "48k"], wav=["-c:a", "pcm_s16le"])[fmt]
        meta = []
        for k, v in j["tags"].items():
            if v != "":
                meta += ["-metadata", f"{k}={v}"]
        cmd = list(base)
        art = j["art_file"]
        if art and fmt in ("mp3", "flac", "m4a"):
            cmd += ["-i", art, "-map", "0:a", "-map", "1:v", "-c:v", "copy", "-disposition:v", "attached_pic"]
        elif art and fmt in ("opus", "ogg"):
            data = open(art, "rb").read()
            mime = "image/jpeg" if art.endswith("jpg") else "image/png"
            blk = base64.b64encode(flac_picture_block(data, mime)).decode()
            meta += ["-metadata", f"METADATA_BLOCK_PICTURE={blk}"]
        cmd += codec + ["-ac", "1"]
        if j["none"] or j["v1"]:
            cmd += ["-map_metadata", "-1"]
            if fmt == "mp3":
                cmd += ["-id3v2_version", "0"]
            meta = []
        elif fmt == "mp3":
            cmd += ["-id3v2_version", "3" if j["freq"] % 2 else "4"]
        cmd += meta + [j["path"]]
        run(cmd)
        if j["v1"]:
            # ffmpeg does not write ID3v1: append the 128-byte tag ourselves (title, artist, album, year, comment, track, genre).
            t = j["tags"]
            f30 = lambda v: v.encode("latin-1", "replace")[:30].ljust(30, b"\0")
            tag = b"TAG" + f30(t["title"]) + f30(t["artist"]) + f30(t["album"]) + t["date"].encode()[:4].ljust(4, b"\0") + b"\0" * 28 + b"\0" + bytes([int(t["track"])]) + bytes([17])
            assert len(tag) == 128
            with open(j["path"], "ab") as fh:
                fh.write(tag)

    with ThreadPoolExecutor(max_workers=8) as ex:
        list(ex.map(render, jobs))
    for f in os.listdir(covers):
        os.remove(os.path.join(covers, f))
    with open(os.path.join(out, "expected.json"), "w", encoding="utf-8") as f:
        json.dump(dict(albums=expected, track_count=len(jobs)), f, ensure_ascii=False, indent=1)
    with open(stamp, "w") as f:
        f.write(f"{VERSION}-{me}\n")
    print(f"library fixtures: {len(jobs)} tracks in {out}")


if __name__ == "__main__":
    main()
