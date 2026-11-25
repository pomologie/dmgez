"""Generate all ten Apple .iconset PNGs and independently check generated ICNS.

The names follow Apple's Icon Set Type reference:
https://developer.apple.com/library/archive/documentation/Xcode/Reference/xcode_ref-Asset_Catalog_Format/IconSetType.html
Element types and planar packing follow the observed Apple iconutil fixture and:
https://github.com/relikd/icns-analysis
"""

import argparse
import struct
import sys
import zlib
from pathlib import Path
from typing import NamedTuple


class Slot(NamedTuple):
    key: str
    filename: str
    pixels: int
    kind: bytes


SLOTS = (
    Slot("16", "icon_16x16.png", 16, b"ic04"),
    Slot("16@2x", "icon_16x16@2x.png", 32, b"ic11"),
    Slot("32", "icon_32x32.png", 32, b"ic05"),
    Slot("32@2x", "icon_32x32@2x.png", 64, b"ic12"),
    Slot("128", "icon_128x128.png", 128, b"ic07"),
    Slot("128@2x", "icon_128x128@2x.png", 256, b"ic13"),
    Slot("256", "icon_256x256.png", 256, b"ic08"),
    Slot("256@2x", "icon_256x256@2x.png", 512, b"ic14"),
    Slot("512", "icon_512x512.png", 512, b"ic09"),
    Slot("512@2x", "icon_512x512@2x.png", 1024, b"ic10"),
)

ELEMENT_KINDS = tuple(slot.kind for slot in SLOTS)
ARGB_KINDS = {b"ic04", b"ic05"}
ICNS_MAGIC = b"icns"
ARGB_MAGIC = b"ARGB"
ICNS_RECORD = struct.Struct(">4sI")
ARGB_CHANNEL_ORDER = (3, 0, 1, 2)
RGBA_CHANNELS = len(ARGB_CHANNEL_ORDER)
MAX_LITERAL_CONTROL = 127
REPEAT_CONTROL_BIAS = 125
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
RGBA_COLOR_TYPE = 6
CHANNEL_BITS = 8
NO_SCANLINE_FILTER = b"\x00"


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


def rgba_rows(slot, index):
    """Build two scanlines for asymmetric quadrants with a unique per-slot palette."""
    half = slot.pixels // 2
    marker = 11 + index * 17
    opaque = bytes((marker, 19, 71, 255))
    translucent = bytes((23, marker, 211, 160))
    light = bytes((83, 197, marker, 255))
    dark = bytes((marker, 37, 17, 224))
    return opaque * half + translucent * half, light * half + dark * half


def png(slot, index):
    top, bottom = rgba_rows(slot, index)
    half = slot.pixels // 2
    header = struct.pack(">IIBBBBB", slot.pixels, slot.pixels, CHANNEL_BITS, RGBA_COLOR_TYPE, 0, 0, 0)
    rows = (NO_SCANLINE_FILTER + top) * half + (NO_SCANLINE_FILTER + bottom) * half
    return (
        PNG_SIGNATURE
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )


def write_pngs(directory):
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    for index, slot in enumerate(SLOTS):
        (directory / slot.filename).write_bytes(png(slot, index))


def unpack_argb(payload, pixels):
    assert payload.startswith(ARGB_MAGIC)
    packed = iter(payload[len(ARGB_MAGIC):])
    for _ in ARGB_CHANNEL_ORDER:
        plane = bytearray()
        while len(plane) < pixels:
            control = next(packed)
            if control <= MAX_LITERAL_CONTROL:
                plane.extend(next(packed) for _ in range(control + 1))
            else:
                plane.extend(bytes([next(packed)]) * (control - REPEAT_CONTROL_BIAS))
        assert len(plane) == pixels
        yield bytes(plane)
    assert next(packed, None) is None


def verify_icns(directory, data):
    kind, length = ICNS_RECORD.unpack_from(data)
    assert kind == ICNS_MAGIC and length == len(data)
    offset = ICNS_RECORD.size
    for index, slot in enumerate(SLOTS):
        kind, length = ICNS_RECORD.unpack_from(data, offset)
        assert kind == slot.kind, (slot, kind)
        stop = offset + length
        assert offset + ICNS_RECORD.size < stop <= len(data)
        payload = data[offset + ICNS_RECORD.size:stop]
        if kind in ARGB_KINDS:
            top, bottom = rgba_rows(slot, index)
            half = slot.pixels // 2
            rgba = top * half + bottom * half
            expected = tuple(rgba[channel::RGBA_CHANNELS] for channel in ARGB_CHANNEL_ORDER)
            assert tuple(unpack_argb(payload, slot.pixels ** 2)) == expected, slot
        else:
            assert payload == (directory / slot.filename).read_bytes(), slot
        offset = stop
    assert offset == len(data), "unexpected elements after the final PNG slot"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--verify", action="store_true")
    options = parser.parse_args()
    if options.verify:
        verify_icns(options.directory, sys.stdin.buffer.read())
    else:
        write_pngs(options.directory)
        for slot in SLOTS:
            print(f'"{slot.key}" = "{slot.filename}"')
