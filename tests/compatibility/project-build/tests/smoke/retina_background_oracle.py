"""Check the two classic TIFF directories and decode their RGB pixel strips.

Field layout and tags follow sections 2, 8, and 18 of the Adobe TIFF 6.0 specification.
https://printtechnologies.org/standards/files/tiff-v6.pdf
"""

from fractions import Fraction
import struct
import sys
from zlib import decompress


TIFF_MAGIC = 42
HEADER_BYTES = 8
IFD_ENTRY_BYTES = 12
INLINE_VALUE_BYTES = 4
BYTE_ORDER = {b"II": "<", b"MM": ">"}
TIFF_SHORT, TIFF_LONG, TIFF_RATIONAL = 3, 4, 5
FIELD_LAYOUT = {
    TIFF_SHORT: ("H", 2),
    TIFF_LONG: ("I", 4),
    TIFF_RATIONAL: ("II", 8),
}
IMAGE_WIDTH = 256
IMAGE_LENGTH = 257
BITS_PER_SAMPLE = 258
COMPRESSION = 259
PHOTOMETRIC_INTERPRETATION = 262
STRIP_OFFSETS = 273
ORIENTATION = 274
SAMPLES_PER_PIXEL = 277
ROWS_PER_STRIP = 278
STRIP_BYTE_COUNTS = 279
X_RESOLUTION = 282
Y_RESOLUTION = 283
PLANAR_CONFIGURATION = 284
RESOLUTION_UNIT = 296
EXTRA_SAMPLES = 338
UNCOMPRESSED, RGB, TOP_LEFT, CHUNKY, INCH, UNASSOCIATED_ALPHA = 1, 2, 1, 1, 2, 2
DEFLATE = 8

BASE_PIXELS = bytes((24, 52, 75)) * 4
RETINA_PIXELS = bytes(
    channel for pixel in (
        (255, 0, 0, 255), (0, 255, 0, 255), (0, 0, 255, 255), (255, 255, 255, 255),
        (255, 255, 0, 128), (0, 255, 255, 128), (255, 0, 255, 128), (0, 0, 0, 128),
        (255, 0, 0, 0), (0, 255, 0, 64), (0, 0, 255, 192), (24, 52, 75, 255),
        (16, 32, 64, 255), (32, 64, 128, 255), (64, 128, 16, 255), (128, 16, 32, 255),
    ) for channel in pixel
)


class ClassicTiff:
    def __init__(self, data):
        self.data = data
        self.order = BYTE_ORDER[data[:2]]
        magic, self.first_directory = self.unpack("HI", 2)
        assert magic == TIFF_MAGIC

    def unpack(self, fields, offset):
        return struct.unpack_from(self.order + fields, self.data, offset)

    def directory(self, offset):
        count, = self.unpack("H", offset)
        cursor = offset + struct.calcsize("H")
        fields = {}
        for _ in range(count):
            tag, kind, amount, value = self.unpack("HHII", cursor)
            if kind in FIELD_LAYOUT:
                pattern, size = FIELD_LAYOUT[kind]
                payload = cursor + IFD_ENTRY_BYTES - INLINE_VALUE_BYTES
                if amount * size > INLINE_VALUE_BYTES:
                    payload = value
                entries = [self.unpack(pattern, payload + n * size) for n in range(amount)]
                fields[tag] = tuple(
                    Fraction(*entry) if kind == TIFF_RATIONAL else entry[0] for entry in entries
                )
            cursor += IFD_ENTRY_BYTES
        next_directory, = self.unpack("I", cursor)
        return fields, next_directory

    def read_representation(self, offset, dimensions, resolution, samples, compression=UNCOMPRESSED):
        width, height = dimensions
        fields, next_directory = self.directory(offset)
        assert fields[IMAGE_WIDTH] == (width,) and fields[IMAGE_LENGTH] == (height,)
        assert fields[X_RESOLUTION] == fields[Y_RESOLUTION] == (resolution,)
        assert fields[RESOLUTION_UNIT] == (INCH,)
        assert fields[PHOTOMETRIC_INTERPRETATION] == (RGB,)
        assert fields[COMPRESSION] == (compression,)
        assert fields.get(ORIENTATION, (TOP_LEFT,)) == (TOP_LEFT,)
        assert fields.get(PLANAR_CONFIGURATION, (CHUNKY,)) == (CHUNKY,)
        assert fields[BITS_PER_SAMPLE] == (8,) * samples
        assert fields[SAMPLES_PER_PIXEL] == (samples,)
        expected_alpha = (UNASSOCIATED_ALPHA,) if samples == 4 else ()
        assert fields.get(EXTRA_SAMPLES, ()) == expected_alpha
        offsets, lengths = fields[STRIP_OFFSETS], fields[STRIP_BYTE_COUNTS]
        rows_per_strip, = fields[ROWS_PER_STRIP]
        assert rows_per_strip > 0
        expected_lengths = tuple(
            min(rows_per_strip, height - first_row) * width * samples
            for first_row in range(0, height, rows_per_strip)
        )
        assert len(offsets) == len(lengths) == len(expected_lengths)
        chunks = []
        for start, length, expected in zip(offsets, lengths, expected_lengths):
            assert length > 0 and HEADER_BYTES <= start <= len(self.data) - length
            payload = self.data[start:start + length]
            if compression == DEFLATE:
                payload = decompress(payload)
            else:
                assert compression == UNCOMPRESSED
            assert len(payload) == expected
            chunks.append(payload)
        return b"".join(chunks), next_directory


if __name__ == "__main__":
    tiff = ClassicTiff(sys.stdin.buffer.read())
    base_pixels, retina = tiff.read_representation(tiff.first_directory, (2, 2), 72, 3)
    assert base_pixels == BASE_PIXELS and retina != 0
    retina_pixels, end = tiff.read_representation(retina, (4, 4), 144, 4)
    assert retina_pixels == RETINA_PIXELS and end == 0
