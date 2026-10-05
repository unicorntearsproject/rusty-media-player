#!/usr/bin/env python3
"""Generate the Rusty Video Player brand assets: the logo and icon SVGs and every raster, .ico, .icns and installer bitmap made from them.

usage: tools/gen-brand.py [--check]

Needs `resvg` (cargo install resvg) on PATH or in ~/.cargo/bin, and Pillow plus fontTools (pip install --user pillow fonttools).
The generated files are committed, so building a package never needs this script; run it when the mark changes.

The mark is original and made of Unicorn Tears design-system tokens (claude-design-system/tokens/colors.css; the same values are
in crates/theme/tokens): a tear drop that points right and so reads as a play button, with three level bars trailing behind it.
It is drawn here as geometry; nothing is traced from the DJ-unicorn art, and it is not a cone.

Outputs (all under the repository root):
  assets/brand/rvp-icon.svg, rvp-icon-small.svg, rvp-icon-maskable.svg, rvp-mark.svg, rvp-logo.svg   the sources
  assets/brand/rvp-icon-512.png, rvp-logo.png                                                           for READMEs and stores
  packaging/icons/hicolor/<n>x<n>/apps/<APP_ID>.png (16 to 512) and scalable/apps/<APP_ID>.svg        freedesktop icon theme
  packaging/icons/rvp.ico, rvp.icns                                                                     Windows and macOS (later)
  packaging/windows/wizard-*.bmp, wizard-small-*.bmp                                                    Inno Setup installer bitmaps
  web/icons/*                                                                                           PWA icons and the favicon
"""
import os, shutil, struct, subprocess, sys, tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
APP_ID = "io.github.idometeor.RustyVideoPlayer"

# Unicorn Tears tokens (claude-design-system/tokens/colors.css).
INK_900, INK_850, INK_800 = "#07060d", "#0c0a16", "#120c1f"
MAGENTA, VIOLET, CYAN = "#ff2bd6", "#9d4eff", "#19e3ff"
NIGHT_TOP = "#2a0f4a"
PINK_WHITE = "#ffeffb"


def defs(uid=""):
    return f"""
    <radialGradient id="night{uid}" cx="50%" cy="-8%" r="120%" fx="50%" fy="-8%">
      <stop offset="0" stop-color="{NIGHT_TOP}"/><stop offset="0.45" stop-color="{INK_800}"/><stop offset="1" stop-color="{INK_900}"/>
    </radialGradient>
    <linearGradient id="tears{uid}" x1="150" y1="130" x2="430" y2="382" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="{MAGENTA}"/><stop offset="0.52" stop-color="{VIOLET}"/><stop offset="1" stop-color="{CYAN}"/>
    </linearGradient>
    <linearGradient id="bars{uid}" x1="0" y1="1" x2="0" y2="0">
      <stop offset="0" stop-color="{CYAN}"/><stop offset="1" stop-color="{VIOLET}"/>
    </linearGradient>
    <radialGradient id="haze{uid}" cx="50%" cy="50%" r="50%">
      <stop offset="0" stop-color="{VIOLET}" stop-opacity="0.50"/><stop offset="1" stop-color="{VIOLET}" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="gloss{uid}" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#ffffff" stop-opacity="0.55"/><stop offset="1" stop-color="#ffffff" stop-opacity="0"/>
    </linearGradient>
    <filter id="glow{uid}" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur stdDeviation="16"/></filter>
    <filter id="glow-s{uid}" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur stdDeviation="9"/></filter>
"""


# The drop: a circle of radius R centred at C whose tangents meet at the tip T (to the right), so it points like a play button.
# Stroked with the same gradient and round joins it gets a soft tip.
import math

C = (262.0, 256.0)
R = 84.0
TIP = (426.0, 256.0)
STROKE = 40.0


def drop_path(scale=1.0, dx=0.0, dy=0.0, c=C, r=R, tip=TIP):
    d = tip[0] - c[0]
    a = math.acos(r / d)
    px, py = c[0] + r * math.cos(a), r * math.sin(a)
    f = lambda x, y: (dx + (x - 256) * scale + 256, dy + (y - 256) * scale + 256)
    t, u, l = f(*tip), f(px, c[1] - py), f(px, c[1] + py)
    rr = r * scale
    return f"M{t[0]:.2f} {t[1]:.2f}L{u[0]:.2f} {u[1]:.2f}A{rr:.2f} {rr:.2f} 0 1 0 {l[0]:.2f} {l[1]:.2f}Z"


BARS = [(66, 56), (102, 104), (138, 152)]  # x, height; width 22, centred on y = 256


def bars(extra=""):
    out = []
    for i, (x, h) in enumerate(BARS):
        out.append(
            f'<rect x="{x}" y="{256 - h / 2}" width="22" height="{h}" rx="11" fill="url(#bars)" opacity="{0.55 + 0.2 * i:.2f}"{extra}/>'
        )
    return "\n    ".join(out)


def mark_group(with_bars=True, glow=True, scale=1.0, dx=0.0, dy=0.0):
    p = drop_path(scale, dx, dy)
    g = []
    if glow:
        g.append(f'<path d="{p}" fill="url(#tears)" stroke="url(#tears)" stroke-width="{STROKE * scale}" stroke-linejoin="round" filter="url(#glow)" opacity="0.65"/>')
    if with_bars:
        g.append(f'<g transform="translate({dx} {dy}) translate(256 256) scale({scale}) translate(-256 -256)">\n    {bars()}\n  </g>')
    g.append(f'<path d="{p}" fill="url(#tears)" stroke="url(#tears)" stroke-width="{STROKE * scale}" stroke-linejoin="round"/>')
    # A soft highlight on the upper left of the drop's round end.
    cx, cy = dx + (C[0] - 256 - 18) * scale + 256, dy + (C[1] - 256 - 34) * scale + 256
    g.append(f'<ellipse cx="{cx:.1f}" cy="{cy:.1f}" rx="{44 * scale:.1f}" ry="{22 * scale:.1f}" transform="rotate(-28 {cx:.1f} {cy:.1f})" fill="url(#gloss)"/>')
    return "\n  ".join(g)


def svg(body, w=512, h=512, uid=""):
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">\n  <defs>{defs(uid)}  </defs>\n  {body}\n</svg>\n'


def icon_svg():
    return svg(
        f'''<rect width="512" height="512" rx="112" fill="url(#night)"/>
  <rect x="1.5" y="1.5" width="509" height="509" rx="110.5" fill="none" stroke="#ffffff" stroke-opacity="0.10" stroke-width="3"/>
  <ellipse cx="270" cy="270" rx="230" ry="190" fill="url(#haze)"/>
  {mark_group(True, True, 1.0, -2, 0)}'''
    )


def icon_small_svg():
    # No bars and no glow: at 16 to 32 px the drop alone has to carry the icon, and it can be bigger.
    return svg(
        f'''<rect width="512" height="512" rx="104" fill="url(#night)"/>
  {mark_group(False, False, 1.45, -66, 0)}'''
    )


def icon_maskable_svg():
    # Full-bleed square; the mark stays inside the central 80 percent (the safe zone of a maskable icon).
    return svg(
        f'''<rect width="512" height="512" fill="url(#night)"/>
  <ellipse cx="270" cy="270" rx="230" ry="190" fill="url(#haze)"/>
  {mark_group(True, True, 0.78, 8, 0)}'''
    )


def mark_svg():
    return svg(mark_group(True, False, 1.0, -2, 0))


def text_path(text, font_path, size, x, y):
    """The text as one SVG path (outlines from the bundled Space Grotesk, so it renders the same everywhere)."""
    from fontTools.pens.svgPathPen import SVGPathPen
    from fontTools.pens.transformPen import TransformPen
    from fontTools.ttLib import TTFont

    font = TTFont(font_path)
    gs = font.getGlyphSet()
    cmap = font.getBestCmap()
    upm = font["head"].unitsPerEm
    k = size / upm
    pen = SVGPathPen(gs, ntos=lambda v: f"{v:.2f}")
    pos = x
    for ch in text:
        name = cmap.get(ord(ch))
        if name is None:
            continue
        gs[name].draw(TransformPen(pen, (k, 0, 0, -k, pos, y)))
        pos += gs[name].width * k
    return pen.getCommands(), pos


def logo_svg():
    font = ROOT / "crates/rvp-ui/assets/fonts/SpaceGrotesk-Bold.ttf"
    d1, w1 = text_path("Rusty Video Player", font, 84, 0, 0)
    w = w1
    ox, oy = 640, 300
    # icon 512 scaled to 0.9 at left; text to the right
    body = f'''<g transform="translate(20 20) scale(0.92)">
    <rect width="512" height="512" rx="112" fill="url(#night)"/>
    <ellipse cx="270" cy="270" rx="230" ry="190" fill="url(#haze)"/>
    {mark_group(True, True, 1.0, -2, 0)}
  </g>
  <path transform="translate(520 292)" d="{d1}" fill="url(#word)"/>
  <text x="522" y="352" font-family="Space Grotesk, Arial, sans-serif" font-size="30" letter-spacing="9" fill="{CYAN}" opacity="0.9"></text>'''
    # The tag line under the name is also outlines.
    tag, tw = text_path("AUDIO + VIDEO, STANDALONE", ROOT / "crates/rvp-ui/assets/fonts/SpaceGrotesk-Medium.ttf", 30, 0, 0)
    body = body.replace(
        f'<text x="522" y="352" font-family="Space Grotesk, Arial, sans-serif" font-size="30" letter-spacing="9" fill="{CYAN}" opacity="0.9"></text>',
        f'<path transform="translate(524 352)" d="{tag}" fill="{CYAN}" opacity="0.92"/>',
    )
    total_w = int(520 + w + 60)
    extra = f'''<linearGradient id="word" x1="520" y1="0" x2="{520 + w:.0f}" y2="0" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="{PINK_WHITE}"/><stop offset="1" stop-color="#ffffff"/></linearGradient>'''
    body = f'<rect width="{total_w}" height="552" rx="72" fill="{INK_900}"/>\n  ' + body
    s = svg(body, total_w, 552, "")
    return s.replace("</defs>", extra + "</defs>", 1)


def wizard_svg(w, h):
    # The tall installer side image: night sky, the mark, the name set small at the bottom.
    font = ROOT / "crates/rvp-ui/assets/fonts/SpaceGrotesk-Bold.ttf"
    k = w / 164
    name, nw = text_path("Rusty", font, 22 * k, 0, 0)
    name2, nw2 = text_path("Video Player", font, 22 * k, 0, 0)
    s = w * 0.88 / 384  # the mark spans x = 66 to 450 of its 512 box
    body = f'''<rect width="{w}" height="{h}" fill="url(#nightw)"/>
  <ellipse cx="{w / 2}" cy="{h * 0.36}" rx="{w * 0.9}" ry="{h * 0.2}" fill="url(#haze)"/>
  <g transform="translate({w / 2 - 258 * s} {h * 0.34 - 256 * s}) scale({s})">
    {mark_group(True, True, 1.0, 0, 0)}
  </g>
  <path transform="translate({w / 2 - nw / 2} {h * 0.74})" d="{name}" fill="#ffffff"/>
  <path transform="translate({w / 2 - nw2 / 2} {h * 0.74 + 28 * k})" d="{name2}" fill="#ffffff"/>
  <rect x="{w * 0.12}" y="{h * 0.74 + 46 * k}" width="{w * 0.76}" height="{3 * k}" rx="{1.5 * k}" fill="url(#tears)" />'''
    extra = f'''<radialGradient id="nightw" cx="50%" cy="0%" r="110%"><stop offset="0" stop-color="{NIGHT_TOP}"/><stop offset="0.5" stop-color="{INK_800}"/><stop offset="1" stop-color="{INK_900}"/></radialGradient>
    <linearGradient id="tears2" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="{MAGENTA}"/><stop offset="0.5" stop-color="{VIOLET}"/><stop offset="1" stop-color="{CYAN}"/></linearGradient>'''
    out = svg(body, w, h)
    out = out.replace('fill="url(#tears)" />', 'fill="url(#tears2)"/>')
    return out.replace("</defs>", extra + "</defs>", 1)


def wizard_small_svg(n):
    k = n / 512
    return svg(f'<rect width="512" height="512" fill="url(#night)"/>\n  {mark_group(False, False, 1.2, -36, 0)}', n, n).replace(
        'width="%d" height="%d" viewBox="0 0 %d %d"' % (n, n, n, n), f'width="{n}" height="{n}" viewBox="0 0 512 512"'
    )


# ---- rendering ------------------------------------------------------------------------------------------------------------

def resvg():
    for c in (shutil.which("resvg"), str(Path.home() / ".cargo/bin/resvg")):
        if c and os.path.exists(c):
            return c
    sys.exit("resvg not found: cargo install resvg")


def render(svg_path, png_path, w=None, h=None):
    cmd = [resvg(), "--background", "transparent"]
    if w:
        cmd += ["-w", str(w)]
    if h:
        cmd += ["-h", str(h)]
    cmd += [str(svg_path), str(png_path)]
    subprocess.run(cmd, check=True)


def write(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    mode = "wb" if isinstance(data, bytes) else "w"
    with open(path, mode) as f:
        f.write(data)


def make_ico(pngs, out):
    # PNG-compressed icon entries (supported since Windows Vista); 256 px is stored as width/height 0.
    entries = []
    for size, png in pngs:
        entries.append((size, png))
    header = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    dirs, blobs = b"", b""
    for size, png in entries:
        dirs += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(png), offset + len(blobs))
        blobs += png
    write(out, header + dirs + blobs)


def make_icns(entries, out):
    body = b""
    for kind, png in entries:
        body += kind + struct.pack(">I", 8 + len(png)) + png
    write(out, b"icns" + struct.pack(">I", 8 + len(body)) + body)


def to_bmp(png, out):
    from PIL import Image

    Path(out).parent.mkdir(parents=True, exist_ok=True)
    im = Image.open(png).convert("RGBA")
    bg = Image.new("RGB", im.size, (7, 6, 13))
    bg.paste(im, mask=im.split()[3])
    bg.save(out, "BMP")


def main():
    brand = ROOT / "assets/brand"
    icon, small, mask, mark, logo = icon_svg(), icon_small_svg(), icon_maskable_svg(), mark_svg(), logo_svg()
    write(brand / "rvp-icon.svg", icon)
    write(brand / "rvp-icon-small.svg", small)
    write(brand / "rvp-icon-maskable.svg", mask)
    write(brand / "rvp-mark.svg", mark)
    write(brand / "rvp-logo.svg", logo)
    tmp = Path(tempfile.mkdtemp(prefix="rvp-brand-"))
    try:
        sizes = [16, 22, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 1024]
        png = {}
        for n in sizes:
            src = brand / ("rvp-icon-small.svg" if n <= 32 else "rvp-icon.svg")
            out = tmp / f"icon-{n}.png"
            render(src, out, n, n)
            png[n] = out.read_bytes()
        icons = ROOT / "packaging/icons"
        for n in [16, 22, 24, 32, 48, 64, 96, 128, 192, 256, 512]:
            write(icons / f"hicolor/{n}x{n}/apps/{APP_ID}.png", png[n])
        write(icons / f"hicolor/scalable/apps/{APP_ID}.svg", icon)
        write(brand / "rvp-icon-512.png", png[512])
        render(brand / "rvp-logo.svg", brand / "rvp-logo.png", h=552)
        # Windows: 16 to 256 (the small variant below 48, as the shell shows those tiny).
        ico_sizes = [16, 24, 32, 48, 64, 128, 256]
        make_ico([(n, png[n] if n in png else None) for n in ico_sizes if n in png], icons / "rvp.ico")
        # macOS (for later): PNG-compressed icns types.
        icns = [(b"icp4", png[16]), (b"icp5", png[32]), (b"icp6", png[64]), (b"ic07", png[128]), (b"ic08", png[256]), (b"ic09", png[512]), (b"ic10", png[1024])]
        # Retina variants (ic11 32@2x of 16, ic12 64@2x of 32, ic13 256@2x of 128, ic14 512@2x of 256).
        icns += [(b"ic11", png[32]), (b"ic12", png[64]), (b"ic13", png[256]), (b"ic14", png[512])]
        make_icns(icns, icons / "rvp.icns")
        # PWA and favicon.
        web = ROOT / "web/icons"
        write(web / "icon.svg", icon)
        write(web / "favicon.svg", small)
        for n in (192, 512):
            write(web / f"icon-{n}.png", png[n])
        mp = tmp / "maskable.png"
        render(brand / "rvp-icon-maskable.svg", mp, 512, 512)
        write(web / "icon-maskable-512.png", mp.read_bytes())
        render(brand / "rvp-icon-maskable.svg", mp, 192, 192)
        write(web / "icon-maskable-192.png", mp.read_bytes())
        # Apple touch icon: opaque and square-cornered (iOS rounds it itself).
        render(brand / "rvp-icon-maskable.svg", mp, 180, 180)
        write(web / "apple-touch-icon.png", mp.read_bytes())
        make_ico([(16, png[16]), (32, png[32]), (48, png[48])], web / "favicon.ico")
        write(web / "favicon-32.png", png[32])
        # Inno Setup bitmaps: the tall side image and the small corner image at 100, 150 and 200 percent.
        win = ROOT / "packaging/windows"
        for scale, w, h in ((100, 164, 314), (150, 246, 472), (200, 328, 628)):
            s = tmp / f"wiz{w}.svg"
            write(s, wizard_svg(w, h))
            p = tmp / f"wiz{w}.png"
            render(s, p, w, h)
            to_bmp(p, win / f"wizard-{w}.bmp")
        for n in (55, 83, 110):
            s = tmp / f"ws{n}.svg"
            write(s, wizard_small_svg(n))
            p = tmp / f"ws{n}.png"
            render(s, p, n, n)
            to_bmp(p, win / f"wizard-small-{n}.bmp")
    finally:
        for f in tmp.iterdir():
            f.unlink()
        tmp.rmdir()
    print("brand assets written")


if __name__ == "__main__":
    main()
