"""Convert the editable geometric SVG into checked-in Windows icon assets.

Run only when editing the logo: python tools/build-logo.py (requires Pillow).
Normal Rust/DX builds consume the generated assets and do not need Python.
The tiny supported SVG subset fails on new geometry instead of silently omitting it.
"""

from pathlib import Path
import xml.etree.ElementTree as ET

from PIL import Image, ImageDraw


def main():
    directory = Path(__file__).resolve().parents[1] / "apps/desktop/assets"
    root = ET.parse(directory / "nyatidraw.svg").getroot()
    factor = 16
    bitmap = Image.new("RGBA", (64 * factor, 64 * factor))
    draw = ImageDraw.Draw(bitmap)
    for element in root:
        kind = element.tag.rsplit("}", 1)[-1]
        if kind == "title":
            continue
        fill = element.attrib["fill"]
        if kind == "rect":
            x, y, width, height, radius = (
                float(element.get(name, "0")) * factor
                for name in ("x", "y", "width", "height", "rx")
            )
            draw.rounded_rectangle((x, y, x + width, y + height), radius, fill)
        elif kind == "polygon":
            points = [
                tuple(float(value) * factor for value in pair.split(","))
                for pair in element.attrib["points"].split()
            ]
            draw.polygon(points, fill)
        else:
            raise ValueError(f"Unsupported SVG element: {kind}")
    bitmap.resize((256, 256), Image.Resampling.LANCZOS).save(directory / "nyatidraw.png")
    bitmap.save(directory / "nyatidraw.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
    rgba = bitmap.resize((64, 64), Image.Resampling.LANCZOS).tobytes()
    (directory / "nyatidraw.rgba").write_bytes(rgba)


if __name__ == "__main__":
    main()
