#!/usr/bin/env python3
"""Generate every raster brand asset of Rusty Wave from the official icon master.

usage: tools/gen-brand.py

Needs Pillow and numpy (pip install --user pillow numpy). The generated files are committed, so building a package never needs this
script; run it when the master changes.

The master is `assets/brand/rusty-wave-icon-master.png` (1254 px, RGBA, transparent: a rusted-metal play triangle with neon magenta and
cyan waves and level bars; the official art from Rusty Bucket's icon set). It is the only source: nothing here redraws it. There is
no vector source, so there is no scalable SVG icon. Sizes come from it with Lanczos resampling on premultiplied alpha followed by a light
unsharp mask. At 32 px and below the full art (wide, with the waves out to both sides) turns to mush, so those sizes use a tighter
square crop around the triangle and the inner waves; it is the same art, only cropped.

Outputs (all under the repository root):
  assets/brand/rusty-wave-icon-512.png, rusty-wave-logo.png      for READMEs and stores
  packaging/icons/hicolor/<n>x<n>/apps/<APP_ID>.png (16 to 512)  freedesktop icon theme
  packaging/icons/rusty-wave.ico, rusty-wave.icns                Windows and macOS (later)
  packaging/windows/wizard-*.bmp, wizard-small-*.bmp             Inno Setup installer bitmaps
  web/icons/*                                                    PWA icons (any, maskable, apple touch) and the favicon
  crates/rvp-ui/assets/logo-<n>.rgba                             the in-app logo, premultiplied RGBA, two sizes (see logo.rs)
  docs/screenshots/brand-sizes.png                               contact sheet of every size
"""
import struct
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent.parent
APP_ID = "io.github.idometeor.RustyWave"
MASTER = ROOT / "assets/brand/rusty-wave-icon-master.png"
FONTS = ROOT / "crates/rvp-ui/assets/fonts"

# Unicorn Tears tokens (claude-design-system/tokens/colors.css; the same values are in crates/theme/tokens).
INK_900, INK_850, INK_800 = (7, 6, 13), (12, 10, 22), (18, 12, 31)
NIGHT_TOP = (42, 15, 74)
MAGENTA, VIOLET, CYAN = (255, 43, 214), (157, 78, 255), (25, 227, 255)
PINK_WHITE = (255, 239, 251)

# The visible art (alpha > 32) spans x 25..1239 and y 153..1094 of the 1254 px master.
FULL = (632.0, 623.5, 1290)  # centre x, centre y, side of the square crop: all the art with a little air
TIGHT = (720.0, 623.5, 960)  # the triangle and the inner waves, for 32 px and below
SMALL_MAX = 32

master = Image.open(MASTER).convert("RGBA")


def crop_square(cx, cy, side):
    side = round(side)
    x0, y0 = round(cx - side / 2), round(cy - side / 2)
    canvas = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    canvas.paste(master, (-x0, -y0))
    return canvas


FULL_ART = crop_square(*FULL)
TIGHT_ART = crop_square(*TIGHT)


def downscale(img, w, h=None, sharpen=60):
    """Lanczos on premultiplied alpha, then a light unsharp mask on the colour (alpha untouched)."""
    h = h or w
    p = img.convert("RGBa").resize((w, h), Image.LANCZOS).convert("RGBA")
    if sharpen:
        rgb = p.convert("RGB").filter(ImageFilter.UnsharpMask(radius=0.6 if w <= 256 else 1.0, percent=sharpen, threshold=0))
        out = rgb.convert("RGBA")
        out.putalpha(p.getchannel("A"))
        p = out
    return p


def icon(n):
    """The app icon at n px: tight crop and a touch more sharpening when tiny."""
    if n <= SMALL_MAX:
        return downscale(TIGHT_ART, n, sharpen=70)
    return downscale(FULL_ART, n, sharpen=50 if n <= 128 else 35)


def png_bytes(img):
    import io

    b = io.BytesIO()
    img.save(b, "PNG", optimize=True)
    return b.getvalue()


def write(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    mode = "wb" if isinstance(data, bytes) else "w"
    with open(path, mode) as f:
        f.write(data)


def save_png(img, path):
    write(path, png_bytes(img))


def make_ico(pngs, out):
    # PNG-compressed icon entries (supported since Windows Vista); 256 px is stored as width/height 0.
    header = struct.pack("<HHH", 0, 1, len(pngs))
    offset = 6 + 16 * len(pngs)
    dirs, blobs = b"", b""
    for size, png in pngs:
        dirs += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(png), offset + len(blobs))
        blobs += png
    write(out, header + dirs + blobs)


def make_icns(entries, out):
    body = b""
    for kind, png in entries:
        body += kind + struct.pack(">I", 8 + len(png)) + png
    write(out, b"icns" + struct.pack(">I", 8 + len(body)) + body)


# ---- backgrounds -----------------------------------------------------------------------------------------------------------

def night(w, h):
    """The Unicorn Tears night sky: a violet glow at the top fading to ink (radial, centred above the top edge)."""
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    # Distance from (w/2, -0.08h), normalised so that 1.0 is 120 percent of the width.
    d = np.sqrt(((xx - w / 2) / (w * 1.2)) ** 2 + ((yy + 0.08 * h) / (h * 1.2)) ** 2)
    stops = [(0.0, NIGHT_TOP), (0.45, INK_800), (1.0, INK_900)]
    out = np.zeros((h, w, 3), np.float32)
    for c in range(3):
        out[..., c] = np.interp(d, [s[0] for s in stops], [s[1][c] for s in stops])
    return Image.fromarray(out.round().astype(np.uint8), "RGB").convert("RGBA")


def art_radius(img):
    """The largest distance of a visible pixel (alpha > 64) from the centre of the square, as a fraction of its side."""
    a = np.array(img.getchannel("A"))
    ys, xs = np.where(a > 64)
    h, w = a.shape
    return float(np.sqrt(((xs - w / 2) ** 2 + (ys - h / 2) ** 2).max()) / w)


def on_night(n, fit):
    """The art on the night background at n px, scaled so its visible pixels stay inside a circle of `fit` * n diameter."""
    k = 4  # supersample
    bg = night(n * k, n * k)
    scale = (fit / 2) / art_radius(FULL_ART)  # side of the art as a fraction of n
    side = max(1, round(n * k * scale))
    art = FULL_ART.resize((side, side), Image.LANCZOS) if side < FULL_ART.width else FULL_ART.resize((side, side), Image.BICUBIC)
    bg.alpha_composite(art, ((n * k - side) // 2, (n * k - side) // 2))
    return downscale(bg, n, sharpen=40).convert("RGB")


def maskable(n):
    # The safe zone is a circle of 80 percent of the icon; keep the art inside 78.
    return on_night(n, 0.78)


def apple_touch(n):
    # iOS rounds the corners itself and has no safe zone: fill more of the square.
    return on_night(n, 0.92)


# ---- text ------------------------------------------------------------------------------------------------------------------

def font(name, px):
    return ImageFont.truetype(str(FONTS / name), round(px))


def text_gradient(img, xy, text, fnt, c0, c1, anchor="la"):
    """Draw text filled with a horizontal gradient from c0 to c1."""
    mask = Image.new("L", img.size, 0)
    ImageDraw.Draw(mask).text(xy, text, font=fnt, fill=255, anchor=anchor)
    box = mask.getbbox()
    if not box:
        return
    x0, x1 = box[0], box[2]
    ramp = np.linspace(0, 1, max(2, x1 - x0), dtype=np.float32)
    grad = np.zeros((img.height, img.width, 4), np.uint8)
    for c in range(3):
        grad[:, x0:x1, c] = np.round(c0[c] + (c1[c] - c0[c]) * ramp)[None, :]
    grad[..., 3] = 255
    layer = Image.fromarray(grad, "RGBA")
    layer.putalpha(mask)
    img.alpha_composite(layer)


def spaced(draw, x, y, text, fnt, fill, tracking):
    for ch in text:
        draw.text((x, y), ch, font=fnt, fill=fill)
        x += draw.textlength(ch, font=fnt) + tracking


# ---- logo and installer art ------------------------------------------------------------------------------------------------

def logo():
    """The horizontal logo: the icon, the name and the tag line on ink, 1330 x 552."""
    w, h = 1330, 552
    img = Image.new("RGBA", (w, h), INK_900 + (255,))
    # rounded card
    mask = Image.new("L", (w, h), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, w - 1, h - 1), radius=72, fill=255)
    card = night(w, h).convert("RGBA")
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    img.paste(card, (0, 0), mask)
    art = downscale(FULL_ART, 500)
    img.alpha_composite(art, (24, 26))
    name_font = font("SpaceGrotesk-Bold.ttf", 128)
    text_gradient(img, (548, 284), "Rusty Wave", name_font, PINK_WHITE, (255, 255, 255), anchor="ls")
    d = ImageDraw.Draw(img)
    spaced(d, 554, 312, "AUDIO + VIDEO, STANDALONE", font("SpaceGrotesk-Medium.ttf", 30), CYAN + (235,), 7)
    # Tears gradient rule under the tag line.
    rw = round(d.textlength("Rusty Wave", font=name_font))
    rule = np.zeros((6, rw, 4), np.uint8)
    t = np.linspace(0, 1, rw)
    for c in range(3):
        rule[..., c] = np.interp(t, [0, 0.5, 1], [MAGENTA[c], VIOLET[c], CYAN[c]])[None, :]
    rule[..., 3] = 255
    img.alpha_composite(Image.fromarray(rule, "RGBA"), (548, 366))
    return img


def wizard_side(w, h):
    k = 4
    W, H = w * k, h * k
    img = night(W, H)
    side = round(W * 0.96)
    art = FULL_ART.resize((side, side), Image.LANCZOS)
    img.alpha_composite(art, ((W - side) // 2, round(H * 0.36) - side // 2))
    fnt = font("SpaceGrotesk-Bold.ttf", 26 * w / 164 * k)
    d = ImageDraw.Draw(img)
    tw = d.textlength("Rusty Wave", font=fnt)
    d.text(((W - tw) / 2, H * 0.74), "Rusty Wave", font=fnt, fill=(255, 255, 255, 255))
    rule = np.zeros((max(2, 3 * k * w // 164), round(W * 0.76), 4), np.uint8)
    t = np.linspace(0, 1, rule.shape[1])
    for c in range(3):
        rule[..., c] = np.interp(t, [0, 0.5, 1], [MAGENTA[c], VIOLET[c], CYAN[c]])[None, :]
    rule[..., 3] = 255
    img.alpha_composite(Image.fromarray(rule, "RGBA"), (round(W * 0.12), round(H * 0.74 + 42 * k * w / 164)))
    return downscale(img, w, h, sharpen=30).convert("RGB")


def wizard_small(n):
    k = 4
    img = night(n * k, n * k)
    side = round(n * k * 0.94)
    img.alpha_composite(FULL_ART.resize((side, side), Image.LANCZOS), ((n * k - side) // 2, (n * k - side) // 2))
    return downscale(img, n, sharpen=30).convert("RGB")


# ---- contact sheet ---------------------------------------------------------------------------------------------------------

def contact_sheet(path):
    dark, light = (24, 22, 34), (236, 234, 242)
    pad = 20
    label = font("SpaceGrotesk-Medium.ttf", 15)
    small = font("SpaceGrotesk-Regular.ttf", 13)
    sheet = Image.new("RGBA", (1560, 2000), (10, 9, 16, 255))
    d = ImageDraw.Draw(sheet)
    y = pad

    def title(text):
        nonlocal y
        d.text((pad, y), text, font=label, fill=PINK_WHITE)
        y += 26

    def tile(x, y, im, panel, w=None, h=None, tag=None):
        w, h = w or im.width, h or im.height
        d.rectangle((x, y, x + w - 1, y + h - 1), fill=panel)
        sheet.alpha_composite(im.convert("RGBA"), (x + (w - im.width) // 2, y + (h - im.height) // 2))
        if tag:
            d.text((x + 4, y + h + 3), tag, font=small, fill=(180, 178, 196))

    small_sizes = [16, 22, 24, 32, 48, 64, 96, 128]
    title("hicolor sizes at 1:1 (16 to 32 use the tighter crop), on a dark and a light panel")
    for panel in (dark, light):
        x = pad
        for n in small_sizes:
            tile(x, y, icon(n), panel, 148, 148, f"{n}" if panel == light else None)
            x += 148 + 8
        y += 148 + 24
    title("16 to 48 px at 6x (nearest neighbour), as the pixels are")
    x = pad
    for n in (16, 22, 24, 32, 48):
        z = icon(n).resize((n * 6, n * 6), Image.NEAREST)
        tile(x, y, z, dark, n * 6 + 12, 48 * 6 + 12, f"{n} px")
        x += n * 6 + 12 + 12
    # The same sizes made from the full art, to show why the small ones are cropped.
    for n in (16, 24, 32):
        z = downscale(FULL_ART, n, sharpen=70).resize((n * 6, n * 6), Image.NEAREST)
        tile(x, y, z, dark, n * 6 + 12, 48 * 6 + 12, f"{n} full (not used)")
        x += n * 6 + 12 + 12
    y += 48 * 6 + 12 + 30
    title("192, 256 and 512 px")
    x = pad
    big = 512
    for n in (192, 256, 512):
        tile(x, y, icon(n), dark, n + 16, big + 16)
        d.text((x + 4, y + big + 19), f"{n}", font=small, fill=(180, 178, 196))
        x += n + 16 + 12
    y += big + 16 + 40
    title("PWA maskable 192 (circle = the 80 percent safe zone) and 512, apple touch 180, favicons 32 and 48, Inno Setup side 164x314 and corner 55 and 110")
    x = pad
    m = maskable(192).convert("RGBA")
    ring = ImageDraw.Draw(m)
    ring.ellipse((192 * 0.1, 192 * 0.1, 192 * 0.9, 192 * 0.9), outline=(255, 255, 255, 110))
    items = [(m, dark), (maskable(512).resize((192, 192), Image.LANCZOS), dark), (apple_touch(180), dark),
             (icon(32), light), (icon(48), light), (wizard_side(164, 314), dark), (wizard_small(55), dark), (wizard_small(110), dark)]
    for im, panel in items:
        tile(x, y, im, panel)
        x += im.width + 16
    y += 314 + 24
    sheet = sheet.crop((0, 0, 1560, y))
    save_png(sheet.convert("RGB"), path)


# ---- the in-app logo ---------------------------------------------------------------------------------------------------------

def app_logo(n):
    """Premultiplied RGBA bytes of the full art at n x n (rvp-ui blends it over the screen)."""
    p = FULL_ART.convert("RGBa").resize((n, n), Image.LANCZOS)  # RGBa is premultiplied
    return p.tobytes()


def main():
    brand = ROOT / "assets/brand"
    icons = ROOT / "packaging/icons"
    sizes = [16, 22, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 1024]
    ic = {n: icon(n) for n in sizes}
    png = {n: png_bytes(ic[n]) for n in sizes}
    for n in [16, 22, 24, 32, 48, 64, 96, 128, 192, 256, 512]:
        write(icons / f"hicolor/{n}x{n}/apps/{APP_ID}.png", png[n])
    write(brand / "rusty-wave-icon-512.png", png[512])
    save_png(logo(), brand / "rusty-wave-logo.png")
    # Windows: 16 to 256.
    make_ico([(n, png[n]) for n in (16, 24, 32, 48, 64, 128, 256)], icons / "rusty-wave.ico")
    # macOS (for later): PNG-compressed icns types, with the retina variants.
    icns = [(b"icp4", png[16]), (b"icp5", png[32]), (b"icp6", png[64]), (b"ic07", png[128]), (b"ic08", png[256]),
            (b"ic09", png[512]), (b"ic10", png[1024]),
            (b"ic11", png[32]), (b"ic12", png[64]), (b"ic13", png[256]), (b"ic14", png[512])]
    make_icns(icns, icons / "rusty-wave.icns")
    # PWA and favicon.
    web = ROOT / "web/icons"
    for n in (192, 512):
        write(web / f"icon-{n}.png", png[n])
        save_png(maskable(n), web / f"icon-maskable-{n}.png")
    save_png(apple_touch(180), web / "apple-touch-icon.png")
    make_ico([(16, png[16]), (32, png[32]), (48, png[48])], web / "favicon.ico")
    write(web / "favicon-32.png", png[32])
    # Inno Setup bitmaps: the tall side image and the small corner image at 100, 150 and 200 percent.
    win = ROOT / "packaging/windows"
    for w, h in ((164, 314), (246, 472), (328, 628)):
        wizard_side(w, h).save(win / f"wizard-{w}.bmp", "BMP")
    for n in (55, 83, 110):
        wizard_small(n).save(win / f"wizard-small-{n}.bmp", "BMP")
    # The in-app logo.
    for n in (64, 192):
        write(ROOT / f"crates/rvp-ui/assets/logo-{n}.rgba", app_logo(n))
    contact_sheet(ROOT / "docs/screenshots/brand-sizes.png")
    print("brand assets written")


if __name__ == "__main__":
    main()
