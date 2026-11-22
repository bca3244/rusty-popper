"""Minimal GIMP XCF reader (8-bit RGB/RGBA/gray layers, RLE or zlib tiles).
Writes each layer as a PNG and prints its name, size, offset, opacity,
visibility and mode, plus a flattened composite of the visible layers."""
import struct
import sys
import zlib
from pathlib import Path

from PIL import Image

src, outdir = Path(sys.argv[1]), Path(sys.argv[2])
outdir.mkdir(parents=True, exist_ok=True)
data = src.read_bytes()
assert data[:9] == b"gimp xcf ", "not an XCF"
version = int(data[10:13]) if data[9:10] == b"v" else 0
pos = 14
ptr64 = version >= 11


def u32():
    global pos
    v = struct.unpack_from(">I", data, pos)[0]
    pos += 4
    return v


def ptr():
    global pos
    if ptr64:
        v = struct.unpack_from(">Q", data, pos)[0]
        pos += 8
    else:
        v = struct.unpack_from(">I", data, pos)[0]
        pos += 4
    return v


def props():
    global pos
    out = {}
    while True:
        t, ln = u32(), u32()
        out[t] = data[pos:pos + ln]
        pos += ln
        if t == 0:
            return out


W, H, base = u32(), u32(), u32()
precision = u32() if version >= 4 else 150
img_props = props()
compression = img_props.get(17, b"\x01")[0]
print(f"XCF v{version} {W}x{H} base={base} precision={precision} compression={compression}")
layer_ptrs = []
while True:
    p = ptr()
    if p == 0:
        break
    layer_ptrs.append(p)


def rle_decode(buf, n, bpp):
    """Planar RLE: each channel's bytes in turn."""
    out = bytearray(n * bpp)
    i = 0
    for ch in range(bpp):
        k = 0
        while k < n:
            op = buf[i]; i += 1
            if op >= 128:
                if op == 128:
                    cnt = buf[i] * 256 + buf[i + 1]; i += 2
                else:
                    cnt = 256 - op
                for _ in range(cnt):
                    out[k * bpp + ch] = buf[i]; i += 1; k += 1
            else:
                if op == 127:
                    cnt = buf[i] * 256 + buf[i + 1]; i += 2
                else:
                    cnt = op + 1
                v = buf[i]; i += 1
                for _ in range(cnt):
                    out[k * bpp + ch] = v; k += 1
    return bytes(out)


def read_level(lp, bpp):
    global pos
    pos = lp
    lw, lh = u32(), u32()
    tps = []
    while True:
        p = ptr()
        if p == 0:
            break
        tps.append(p)
    raw = bytearray(lw * lh * bpp)
    tw = (lw + 63) // 64
    for ti, tp in enumerate(tps):
        tx, ty = (ti % tw) * 64, (ti // tw) * 64
        w, h = min(64, lw - tx), min(64, lh - ty)
        end = tps[ti + 1] if ti + 1 < len(tps) else len(data)
        chunk = data[tp:end]
        if compression == 1:
            tile = rle_decode(chunk, w * h, bpp)
        elif compression == 2:
            tile = zlib.decompressobj().decompress(chunk)[: w * h * bpp]
        else:
            tile = chunk[: w * h * bpp]
        for row in range(h):
            dst = ((ty + row) * lw + tx) * bpp
            raw[dst:dst + w * bpp] = tile[row * w * bpp:(row + 1) * w * bpp]
    return lw, lh, bytes(raw)


canvas = Image.new("RGBA", (W, H), (0, 0, 0, 0))
layers = []
for idx, lp in enumerate(layer_ptrs):
    pos = lp
    lw, lh, ltype = u32(), u32(), u32()
    nlen = u32()
    name = data[pos:pos + nlen - 1].decode("utf-8", "replace"); pos += nlen
    lp_ = props()
    hptr = ptr()
    ptr()  # mask
    opacity = lp_.get(6)
    opacity = struct.unpack(">I", opacity)[0] / 255 if opacity else 1.0
    if 33 in lp_:
        opacity = struct.unpack(">f", lp_[33])[0]
    visible = struct.unpack(">I", lp_[8])[0] if 8 in lp_ else 1
    ox, oy = struct.unpack(">ii", lp_[15]) if 15 in lp_ else (0, 0)
    mode = struct.unpack(">I", lp_[7])[0] if 7 in lp_ else 0
    group = 29 in lp_
    item_path = struct.unpack(f">{len(lp_[30]) // 4}I", lp_[30]) if 30 in lp_ else ()
    pos = hptr
    hw, hh, bpp = u32(), u32(), u32()
    lvl = ptr()
    info = f"[{idx}] {name!r} {lw}x{lh} at {ox},{oy} type={ltype} bpp={bpp} opacity={opacity:.2f} visible={visible} mode={mode} group={group} path={item_path}"
    if group or bpp not in (1, 2, 3, 4) or lw == 0:
        print(info + "  (skipped)")
        continue
    lw2, lh2, raw = read_level(lvl, bpp)
    modes = {1: "L", 2: "LA", 3: "RGB", 4: "RGBA"}
    im = Image.frombytes(modes[bpp], (lw2, lh2), raw).convert("RGBA")
    safe = "".join(c if c.isalnum() or c in "-_ " else "_" for c in name)
    im.save(outdir / f"{idx:02d}-{safe}.png")
    print(info)
    layers.append((im, ox, oy, opacity, visible))

# XCF lists layers top first; composite bottom-up (normal mode only).
for im, ox, oy, op, vis in reversed(layers):
    if not vis:
        continue
    if op < 1.0:
        a = im.split()[3].point(lambda v: int(v * op))
        im = im.copy(); im.putalpha(a)
    tmp = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    tmp.paste(im, (ox, oy))
    canvas = Image.alpha_composite(canvas, tmp)
canvas.save(outdir / "composite.png")
print("done")
