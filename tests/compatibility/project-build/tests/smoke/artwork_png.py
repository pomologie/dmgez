"""Read 8-bit RGB/RGBA test artwork independently of the Rust PNG codec.

The framing and scanline filters follow the PNG specification
(https://www.w3.org/TR/png-3/).
"""

from struct import Struct
from zlib import crc32, decompress

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
PNG_CHUNK = Struct(">I4s")
PNG_HEADER = Struct(">IIBBBBB")
PNG_CRC = Struct(">I")
CHANNELS_BY_COLOR_TYPE = {2: 3, 6: 4}
NO_FILTER, SUB_FILTER, UP_FILTER, AVERAGE_FILTER, PAETH_FILTER = range(5)
BYTE_MASK = 0xFF


def paeth(left, up, diagonal):
    estimate = left + up - diagonal
    distances = (abs(estimate - left), abs(estimate - up), abs(estimate - diagonal))
    return (left, up, diagonal)[distances.index(min(distances))]


class ArtworkPng:
    def __init__(self, data, dimensions):
        assert data.startswith(PNG_SIGNATURE)
        offset = len(PNG_SIGNATURE)
        compressed = bytearray()
        while offset < len(data):
            size, tag = PNG_CHUNK.unpack_from(data, offset)
            start = offset + PNG_CHUNK.size
            end = start + size
            content = data[start:end]
            checksum, = PNG_CRC.unpack_from(data, end)
            assert checksum == crc32(tag + content), tag
            if tag == b"IHDR":
                width, height, depth, color, compression, filtering, interlaced = PNG_HEADER.unpack(content)
                assert (width, height) == dimensions
                assert (depth, compression, filtering, interlaced) == (8, 0, 0, 0)
                self.width = width
                self.channels = CHANNELS_BY_COLOR_TYPE[color]
            elif tag == b"IDAT":
                compressed.extend(content)
            offset = end + PNG_CRC.size
            if tag == b"IEND":
                assert not content and offset == len(data)
                break
        else:
            raise AssertionError("missing IEND")

        stride = self.width * self.channels
        encoded = decompress(compressed)
        assert len(encoded) == (stride + 1) * height
        previous = bytes(stride)
        self.rows = []
        for offset in range(0, len(encoded), stride + 1):
            mode = encoded[offset]
            row = bytearray(encoded[offset + 1:offset + 1 + stride])
            assert mode in (NO_FILTER, SUB_FILTER, UP_FILTER, AVERAGE_FILTER, PAETH_FILTER)
            if mode != NO_FILTER:
                for index in range(stride):
                    left = row[index - self.channels] if index >= self.channels else 0
                    up = previous[index]
                    if mode == SUB_FILTER:
                        delta = left
                    elif mode == UP_FILTER:
                        delta = up
                    elif mode == AVERAGE_FILTER:
                        delta = (left + up) // 2
                    else:
                        diagonal = previous[index - self.channels] if index >= self.channels else 0
                        delta = paeth(left, up, diagonal)
                    row[index] = (row[index] + delta) & BYTE_MASK
            self.rows.append(row)
            previous = row

    def pixel(self, x, y):
        start = x * self.channels
        values = tuple(self.rows[y][start:start + self.channels])
        return values + (BYTE_MASK,) if self.channels == 3 else values
