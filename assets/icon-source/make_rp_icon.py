"""Builds the RP monogram icon on Brian's Wheel_1000x1000 template:
template ring layer (Chrome / Copper / ...) over an ember disc cut to the
template's Background shape, with chrome "RP" letters between them so the
ring's glass highlights sit on top, like a real wheel image."""
import math
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

T = Path(sys.argv[1])            # folder with the extracted template layers
OUT = Path(sys.argv[2])
OUT.mkdir(parents=True, exist_ok=True)
S = 1000
IMPACT = r"C:\Windows\Fonts\impact.ttf"
BAHN = r"C:\Windows\Fonts\bahnschrift.ttf"


def place(layer_png, ox, oy):
    im = Image.open(layer_png).convert("RGBA")
    canvas = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    canvas.paste(im, (ox, oy))
    return canvas


bg_shape = place(T / "04-Background.png", 112, 108).split()[3]
cx, cy = 500, 515  # centre of the template disc


def ember_disc():
    # Dark red-orange glow behind the letters, falling to near black at the rim.
    disc = Image.new("RGBA", (S, S))
    px = disc.load()
    for y in range(S):
        for x in range(S):
            r = math.hypot(x - cx, (y - cy) * 1.05) / 390
            g = math.exp(-(r * 1.6) ** 2)
            px[x, y] = (int(20 + 150 * g), int(8 + 50 * g), int(10 + 12 * g), 255)
    disc.putalpha(bg_shape)
    return disc


def chrome_text(text, font_path, size, center, top_col, bot_col, stroke, stroke_col, glow=None, glow_blur=24):
    font = ImageFont.truetype(font_path, size)
    layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    bbox = d.textbbox((0, 0), text, font=font, stroke_width=stroke)
    w, h = bbox[2] - bbox[0], bbox[3] - bbox[1]
    x, y = center[0] - w / 2 - bbox[0], center[1] - h / 2 - bbox[1]
    if glow:
        g = Image.new("L", (S, S), 0)
        ImageDraw.Draw(g).text((x, y), text, font=font, fill=255, stroke_width=stroke + 10, stroke_fill=255)
        g = g.filter(ImageFilter.GaussianBlur(glow_blur))
        gl = Image.new("RGBA", (S, S), glow)
        gl.putalpha(g.point(lambda v: v * glow[3] // 255))
        layer = Image.alpha_composite(layer, gl)
        d = ImageDraw.Draw(layer)
    d.text((x, y), text, font=font, fill=stroke_col + (255,), stroke_width=stroke, stroke_fill=stroke_col + (255,))
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).text((x, y), text, font=font, fill=255)
    top, bot = y + bbox[1] + stroke, y + bbox[3] - stroke
    grad = Image.new("RGBA", (S, S))
    gp = grad.load()
    for yy in range(S):
        t = max(0.0, min(1.0, (yy - top) / max(1, bot - top)))
        # Polished metal: bright band near the top, darker middle, lift at the bottom.
        k = 1.0 + 0.45 * math.exp(-((t - 0.22) ** 2) / 0.004) - 0.25 * math.exp(-((t - 0.55) ** 2) / 0.01)
        col = tuple(max(0, min(255, int((top_col[i] * (1 - t) + bot_col[i] * t) * k))) for i in range(3))
        for xx in range(S):
            gp[xx, yy] = col + (255,)
    layer.paste(grad, (0, 0), mask)
    return layer


def drop_shadow(img, off=10, blur=12, alpha=170):
    a = img.split()[3].filter(ImageFilter.GaussianBlur(blur)).point(lambda v: v * alpha // 255)
    sh = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    sh.paste(Image.new("RGBA", (S, S), (0, 0, 0, 255)), (off, off), a)
    return sh


disc = ember_disc()
rp = chrome_text("RP", IMPACT, 440, (cx, cy - 40), (250, 250, 252), (125, 130, 140), 18, (22, 10, 4), glow=(255, 110, 25, 230))
sub = chrome_text("RUSTY POPPER", BAHN, 62, (cx, 735), (255, 225, 160), (225, 110, 35), 7, (22, 10, 4))

for ring_name, ring_file in (("chrome", "01-Chrome.png"), ("copper", "02-Copper.png")):
    ring = place(T / ring_file, 66, 68)
    out = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    for layer in (disc, drop_shadow(rp), rp, drop_shadow(sub, 6, 8), sub, ring):
        out = Image.alpha_composite(out, layer)
    out.save(OUT / f"rp-{ring_name}-1000.png")

# Review sheet: both rings at 256, 64, 32, 16 px.
sizes = [256, 64, 32, 16]
sheet = Image.new("RGBA", (760, 640), (24, 26, 34, 255))
d = ImageDraw.Draw(sheet)
lf = ImageFont.truetype(BAHN, 26)
for i, ring_name in enumerate(("chrome", "copper")):
    img = Image.open(OUT / f"rp-{ring_name}-1000.png")
    y = 20 + i * 310
    d.text((20, y), f"RP monogram, {ring_name} ring", font=lf, fill=(230, 232, 240, 255))
    x = 20
    for sz in sizes:
        im = img.resize((sz, sz), Image.LANCZOS)
        sheet.alpha_composite(im, (x, y + 40 + (256 - sz) // 2))
        x += sz + 40
sheet.save(OUT / "rp-review-sheet.png")
print("ok")
