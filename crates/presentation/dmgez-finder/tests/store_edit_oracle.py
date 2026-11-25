"""Read edited Finder stores with the independent ds_store reader."""

from importlib.metadata import version
from io import BytesIO
import sys

from ds_store import DSStore

assert version("ds_store") == "1.3.4"
assert version("mac_alias") == "2.2.3"

with DSStore.open(BytesIO(sys.stdin.buffer.read()), "r") as store:
    assert store["bam"]["Iloc"] == (530, 180)
    if sys.argv[1] == "native":
        assert len(list(store)) == 3
        assert store["bar"]["Iloc"] == (420, 55)
        assert store["baz"]["Iloc"] == (454, 124)
    else:
        assert sys.argv[1] == "synthetic"
        assert len(list(store)) == 2
        assert store["zz-opaque"]["ext1"] == (b"blob", b"\x00\xffA\xa5\x00")
