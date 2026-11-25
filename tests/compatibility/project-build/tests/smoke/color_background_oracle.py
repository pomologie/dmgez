"""Read an HFS+ project's solid Finder color using the independent DS store reader."""

from importlib.metadata import version
from io import BytesIO
from math import isclose
import sys

from ds_store import DSStore

STORE_NAME = "."
ICON_VIEW = "icvp"
BACKGROUND_TYPE = "backgroundType"
SOLID_BACKGROUND = 1
BACKGROUND_ALIAS = "backgroundImageAlias"
RGB_CHANNELS = ("backgroundColorRed", "backgroundColorGreen", "backgroundColorBlue")
RGB_ABS_TOLERANCE = 1e-12

assert version("ds_store") == "1.3.4"
label, *channels = sys.argv[1:]
assert len(channels) == len(RGB_CHANNELS)

with DSStore.open(BytesIO(sys.stdin.buffer.read())) as store:
    icons = store[STORE_NAME][ICON_VIEW]
    assert type(icons[BACKGROUND_TYPE]) is int
    assert icons[BACKGROUND_TYPE] == SOLID_BACKGROUND
    assert BACKGROUND_ALIAS not in icons
    assert len(channels) == len(RGB_CHANNELS)
    for key, expected in zip(RGB_CHANNELS, channels):
        actual = icons[key]
        assert type(actual) is float, (label, key, actual)
        assert isclose(actual, float(expected), abs_tol=RGB_ABS_TOLERANCE, rel_tol=0), (
            label, key, actual, expected
        )
