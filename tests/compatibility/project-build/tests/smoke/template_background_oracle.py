"""Check both physical renditions of the built-in arrow."""

import sys

from retina_background_oracle import ClassicTiff, DEFLATE

CANVAS = (640, 280)
RESOLUTIONS = (72, 144)
CHANNELS = 3
NEUTRAL = (246, 248, 251)
ARROW = (30, 99, 174)
SAMPLES = (
    ((20, 20), NEUTRAL),
    ((320, 140), ARROW),
    ((396, 120), NEUTRAL),
    ((396, 140), ARROW),
)


def verify_arrow(data):
    tiff = ClassicTiff(data)
    offset = tiff.first_directory
    for scale, density in enumerate(RESOLUTIONS, start=1):
        dimensions = tuple(points * scale for points in CANVAS)
        pixels, offset = tiff.read_representation(offset, dimensions, density, CHANNELS, DEFLATE)
        for (x, y), expected in SAMPLES:
            start = (y * scale * dimensions[0] + x * scale) * CHANNELS
            assert tuple(pixels[start:start + CHANNELS]) == expected, (scale, x, y)
    assert offset == 0


if __name__ == "__main__":
    verify_arrow(sys.stdin.buffer.read())
