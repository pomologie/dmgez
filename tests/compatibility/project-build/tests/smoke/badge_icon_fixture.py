"""Generate a synthetic badge and check its published ICNS independently."""

import argparse
import sys
from pathlib import Path
from zlib import compress

from artwork_png import ArtworkPng, PNG_HEADER, PNG_SIGNATURE
from volume_icon_fixture import (
    ARGB_KINDS,
    CHANNEL_BITS,
    ICNS_MAGIC,
    ICNS_RECORD,
    NO_SCANLINE_FILTER,
    RGBA_COLOR_TYPE,
    SLOTS,
    chunk,
    unpack_argb,
)

BADGE_SIDE = 32
BADGE_RGBA = (221, 28, 118, 255)
FALLBACK_RGBA = (22, 193, 141, 255)
BADGE_ICNS_SLOTS = ("32@2x", "16@2x")
GEOMETRY_GRID = 1024
BADGE_CENTER = 736


def solid_badge_png(side, rgba):
    header = PNG_HEADER.pack(side, side, CHANNEL_BITS, RGBA_COLOR_TYPE, 0, 0, 0)
    rows = (NO_SCANLINE_FILTER + bytes(rgba) * side) * side
    return (
        PNG_SIGNATURE + chunk(b"IHDR", header) + chunk(b"IDAT", compress(rows)) + chunk(b"IEND", b"")
    )


def write_badge_png(path):
    Path(path).write_bytes(solid_badge_png(BADGE_SIDE, BADGE_RGBA))


def write_badge_icns(path):
    slots = {slot.key: slot for slot in SLOTS}
    elements = []
    for key, rgba in zip(BADGE_ICNS_SLOTS, (BADGE_RGBA, FALLBACK_RGBA)):
        slot = slots[key]
        payload = solid_badge_png(slot.pixels, rgba)
        elements.append(ICNS_RECORD.pack(slot.kind, ICNS_RECORD.size + len(payload)) + payload)
    body = b"".join(elements)
    Path(path).write_bytes(ICNS_RECORD.pack(ICNS_MAGIC, ICNS_RECORD.size + len(body)) + body)


def verify_badged_icns(data):
    assert ICNS_RECORD.unpack_from(data) == (ICNS_MAGIC, len(data))
    offset = ICNS_RECORD.size
    for slot in SLOTS:
        kind, size = ICNS_RECORD.unpack_from(data, offset)
        assert kind == slot.kind, (slot, kind)
        end = offset + size
        assert offset + ICNS_RECORD.size < end <= len(data)
        payload = data[offset + ICNS_RECORD.size:end]
        center = slot.pixels * BADGE_CENTER // GEOMETRY_GRID
        if kind in ARGB_KINDS:
            alpha, red, green, blue = unpack_argb(payload, slot.pixels ** 2)
            index = center * slot.pixels + center
            assert (red[index], green[index], blue[index], alpha[index]) == BADGE_RGBA, slot
            assert alpha[0] == 0, slot
        else:
            image = ArtworkPng(payload, (slot.pixels, slot.pixels))
            assert image.pixel(center, center) == BADGE_RGBA, slot
            assert image.pixel(0, 0)[-1] == 0, slot
        offset = end
    assert offset == len(data), "unexpected additional icon representations"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument("--write", type=Path)
    modes.add_argument("--write-icns", type=Path)
    modes.add_argument("--verify", action="store_true")
    options = parser.parse_args()
    if options.verify:
        verify_badged_icns(sys.stdin.buffer.read())
    elif options.write_icns:
        write_badge_icns(options.write_icns)
    else:
        write_badge_png(options.write)
