"""Make tests/corpus/gen: JPEG files covering the processes and layouts the
decoder must handle, each with a reference rendering, using libjpeg-turbo
as a black box (through the imagecodecs and Pillow wheels; no source of
either was read). Run once; the output is committed. Requires
`pip install imagecodecs pillow numpy`.

Sources: tests/corpus/ijg-testorig.ppm (the IJG test picture, 227x149) and
a synthetic 16-bit gradient for the high-precision processes.

Each <name>.jpg has a <name>.ref: an ASCII header "REF w h channels bits\n"
then the samples, 8-bit as bytes, more as big-endian 16-bit. The reference
is libjpeg-turbo's own decode of the file (RGB for colour, the stored
convention for CMYK); for lossless files it is the exact source.
"""

import hashlib, io, os, sys
import numpy as np
import imagecodecs as ic
from PIL import Image

root = os.path.join(os.path.dirname(__file__), "..", "tests", "corpus")
out = os.path.join(root, "gen")
os.makedirs(out, exist_ok=True)

def read_ppm(p):
    d = open(p, "rb").read()
    parts = d.split(b"\n", 3)
    w, h = map(int, parts[1].split())
    return np.frombuffer(parts[3], np.uint8)[: w * h * 3].reshape(h, w, 3).copy()

rgb = read_ppm(os.path.join(root, "ijg-testorig.ppm"))
grey = np.round(rgb @ np.array([0.299, 0.587, 0.114])).clip(0, 255).astype(np.uint8)
cmyk = np.concatenate([255 - rgb, np.minimum.reduce(255 - rgb, axis=2)[..., None]], axis=2).astype(np.uint8)
h, w = grey.shape
yy, xx = np.mgrid[0:h, 0:w]
def deep(bits):
    m = (1 << bits) - 1
    v = (xx / w * 0.6 + yy / h * 0.3) * m + (grey.astype(np.float64) / 255 * 0.1 * m)
    return np.round(v).clip(0, m).astype(np.uint16)
deep3 = lambda bits: np.stack([deep(bits), deep(bits)[::-1], deep(bits)[:, ::-1]], axis=2)

manifest = []

def ref_write(name, arr, bits):
    arr = np.asarray(arr)
    c = 1 if arr.ndim == 2 else arr.shape[2]
    hdr = f"REF {arr.shape[1]} {arr.shape[0]} {c} {bits}\n".encode()
    body = arr.astype(np.uint8).tobytes() if bits <= 8 else arr.astype(">u2").tobytes()
    open(os.path.join(out, name + ".ref"), "wb").write(hdr + body)

def save(name, data, how, ref=None, bits=8):
    open(os.path.join(out, name + ".jpg"), "wb").write(data)
    if ref is None:
        ref = ic.jpeg8_decode(data)
    ref_write(name, ref, bits)
    manifest.append((name, hashlib.sha256(data).hexdigest(), len(data), how))

E = ic.jpeg8_encode
for s in ["444", "422", "420", "440", "411"]:
    save(f"baseline-{s}", E(rgb, level=75, subsampling=s), f"jpeg8_encode(rgb, level=75, subsampling='{s}')")
save("baseline-q100-444", E(rgb, level=100, subsampling="444"), "jpeg8_encode(rgb, level=100, subsampling='444')")
save("baseline-q5-420", E(rgb, level=5, subsampling="420"), "jpeg8_encode(rgb, level=5, subsampling='420')")
save("optimized-420", E(rgb, level=80, optimize=True), "jpeg8_encode(rgb, level=80, optimize=True)")
save("grey", E(grey, level=80), "jpeg8_encode(grey, level=80)")
save("rgb-adobe", E(rgb, level=85, colorspace="RGB", outcolorspace="RGB"), "jpeg8_encode(rgb, level=85, colorspace='RGB', outcolorspace='RGB')")
save("cmyk-adobe", E(cmyk, level=85, colorspace="CMYK", outcolorspace="CMYK"), "jpeg8_encode(cmyk, level=85, colorspace='CMYK', outcolorspace='CMYK')")
save("ycck-adobe", E(cmyk, level=85, colorspace="CMYK", outcolorspace="YCCK"), "jpeg8_encode(cmyk, level=85, colorspace='CMYK', outcolorspace='YCCK')")
save("ext12-colour", E(deep3(12), level=90, bitspersample=12), "jpeg8_encode(16-bit gradient >> 4, level=90, bitspersample=12)", bits=12)
save("ext12-grey", E(deep(12), level=90, bitspersample=12), "jpeg8_encode(12-bit gradient, level=90, bitspersample=12)", bits=12)
for p in range(1, 8):
    save(f"lossless8-p{p}", E(grey, lossless=True, predictor=p), f"jpeg8_encode(grey, lossless=True, predictor={p})", ref=grey)
save("lossless8-rgb-p6", E(rgb, lossless=True, predictor=6), "jpeg8_encode(rgb, lossless=True, predictor=6)", ref=rgb)
for b in [12, 16]:
    save(f"lossless{b}-p1", E(deep(b), lossless=True, predictor=1, bitspersample=b), f"jpeg8_encode({b}-bit gradient, lossless=True, predictor=1, bitspersample={b})", ref=deep(b), bits=b)
save("lossless12-rgb-p4", E(deep3(12), lossless=True, predictor=4, bitspersample=12), "jpeg8_encode(12-bit RGB gradient, lossless=True, predictor=4, bitspersample=12)", ref=deep3(12), bits=12)

def pil(name, im, how, **kw):
    b = io.BytesIO()
    im.save(b, "JPEG", **kw)
    save(name, b.getvalue(), how)

im = Image.fromarray(rgb)
pil("progressive-420", im, "Pillow save(quality=80, progressive=True)", quality=80, progressive=True)
pil("progressive-444", im, "Pillow save(quality=90, progressive=True, subsampling=0)", quality=90, progressive=True, subsampling=0)
pil("progressive-grey", Image.fromarray(grey), "Pillow save(L, quality=80, progressive=True)", quality=80, progressive=True)
pil("restart-blocks-5", im, "Pillow save(quality=80, restart_marker_blocks=5)", quality=80, restart_marker_blocks=5)
pil("restart-rows-1-progressive", im, "Pillow save(quality=80, progressive=True, restart_marker_rows=1)", quality=80, progressive=True, restart_marker_rows=1)
exif = Image.Exif()
exif[0x0112] = 6
icc = bytes((i * 7) & 0xFF for i in range(150_000))
pil("exif-orientation-6-icc-3-chunks", im, "Pillow save(quality=80, exif=Orientation 6, icc_profile=150000 synthetic bytes)", quality=80, exif=exif.tobytes(), icc_profile=icc)
open(os.path.join(out, "exif-orientation-6-icc-3-chunks.icc"), "wb").write(icc)

with open(os.path.join(out, "MANIFEST.txt"), "w", newline="\n") as f:
    f.write(f"# Made by tools/make_corpus.py with imagecodecs {ic.__version__} ({ic.jpeg8_version()}) and Pillow {Image.__version__}.\n")
    f.write("# name  sha256  bytes  how\n")
    for n, s, l, how in manifest:
        f.write(f"{n}\t{s}\t{l}\t{how}\n")
print(len(manifest), "files")
