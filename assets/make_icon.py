"""Make every size of the launcher icon from the 16x16 pixel-art master.

    python assets/make_icon.py

Reads  assets/icon-16.png  (the master: 16x16 PNG with transparency) and writes
    assets/icon-256.png    window / taskbar icon
    assets/icon.ico        icon of LimeCraft.exe (16, 32, 48, 64, 128, 256 px)

Every size is a whole multiple of 16, so the pixels are enlarged without any
smoothing and stay sharp. Needs Pillow (`pip install pillow`).
"""
import io
import os
import struct

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
SIZES = [16, 32, 48, 64, 128, 256]


def main():
    master = Image.open(os.path.join(HERE, "icon-16.png")).convert("RGBA")
    if master.size != (16, 16):
        raise SystemExit(f"icon-16.png must be 16x16, got {master.size}")

    master.resize((256, 256), Image.NEAREST).save(os.path.join(HERE, "icon-256.png"), optimize=True)

    # an .ico is a small directory followed by the pictures (PNG data is allowed)
    blobs = []
    for s in SIZES:
        buf = io.BytesIO()
        master.resize((s, s), Image.NEAREST).save(buf, format="PNG", optimize=True)
        blobs.append(buf.getvalue())
    out = bytearray(struct.pack("<HHH", 0, 1, len(SIZES)))
    offset = 6 + 16 * len(SIZES)
    for s, blob in zip(SIZES, blobs):
        dim = 0 if s >= 256 else s  # 0 means 256
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
    for blob in blobs:
        out += blob
    with open(os.path.join(HERE, "icon.ico"), "wb") as f:
        f.write(out)
    print("icon-256.png and icon.ico written:", ", ".join(f"{s}px" for s in SIZES))


if __name__ == "__main__":
    main()
