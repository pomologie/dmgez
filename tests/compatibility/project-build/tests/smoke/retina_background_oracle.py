"""Check the two classic TIFF directories and their uncompressed pixel strips.

Field layout and tags follow sections 2, 8, and 18 of the Adobe TIFF 6.0 specification.
https://printtechnologies.org/standards/files/tiff-v6.pdf
"""

from fractions import Fraction
import struct
import sys


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

    def verify_representation(self, offset, dimension, resolution, samples, pixels):
        fields, next_directory = self.directory(offset)
        assert fields[IMAGE_WIDTH] == fields[IMAGE_LENGTH] == (dimension,)
        assert fields[X_RESOLUTION] == fields[Y_RESOLUTION] == (resolution,)
        assert fields[RESOLUTION_UNIT] == (INCH,)
        assert fields[PHOTOMETRIC_INTERPRETATION] == (RGB,)
        assert fields[COMPRESSION] == (UNCOMPRESSED,)
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
            min(rows_per_strip, dimension - first_row) * dimension * samples
            for first_row in range(0, dimension, rows_per_strip)
        )
        assert len(offsets) == len(lengths) and lengths == expected_lengths
        strips = zip(offsets, lengths)
        chunks = []
        for start, length in strips:
            assert HEADER_BYTES <= start <= len(self.data) - length
            chunks.append(self.data[start:start + length])
        actual = b"".join(chunks)
        assert actual == pixels
        return next_directory


tiff = ClassicTiff(sys.stdin.buffer.read())
retina = tiff.verify_representation(tiff.first_directory, 2, 72, 3, BASE_PIXELS)
assert retina != 0
assert tiff.verify_representation(retina, 4, 144, 4, RETINA_PIXELS) == 0
