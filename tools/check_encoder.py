"""Have libjpeg-turbo (a black box, through the imagecodecs and Pillow
wheels) decode every file `cargo run --example encode_matrix` writes, and
compare its pixels with this crate's decode of the same file. Prints one
line per file and a summary. Requires `pip install imagecodecs pillow numpy`.

    cargo run --release --example encode_matrix -- tests/corpus/ijg-testorig.ppm target/matrix
    python tools/check_encoder.py target/matrix
"""
import glob, io, os, sys, warnings
import numpy as np
import imagecodecs as ic
from PIL import Image

d = sys.argv[1]
worst = 0
rows = 0
problems = []
for f in sorted(glob.glob(os.path.join(d, "*.jpg"))):
    data = open(f, "rb").read()
    mine = np.frombuffer(open(f[:-4] + ".rgb", "rb").read(), np.uint8).astype(int)
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        try:
            ref = ic.jpeg8_decode(data)
        except Exception as e:
            problems.append((os.path.basename(f), f"imagecodecs: {e}"))
            continue
        try:
            im = Image.open(io.BytesIO(data)); im.load()
            icc = im.info.get("icc_profile")
        except Exception as e:
            problems.append((os.path.basename(f), f"Pillow: {e}"))
            icc = None
    if ref.ndim == 2:
        ref = np.repeat(ref[..., None], 3, axis=2)
    diff = np.abs(ref.astype(int).reshape(-1) - mine)
    worst = max(worst, diff.max())
    rows += 1
    print(f"{os.path.basename(f):<40} max {diff.max():>2} mean {diff.mean():.3f} icc {len(icc) if icc else 0}")
print(f"{rows} files decoded by libjpeg-turbo {ic.jpeg8_version()}; worst difference from this crate's decode {worst}; problems: {problems}")
