"""Check the rebuilt catalog view with the independent Python reader."""

from importlib.metadata import version
from io import BytesIO
import sys

from ds_store import DSStore

assert version("ds_store") == "1.3.4"
assert version("mac_alias") == "2.2.3"

with DSStore.open(BytesIO(sys.stdin.buffer.read())) as store:
    assert store["."]["icvl"] == (b"type", b"Nlsv")
    assert store["."]["bwsp"]["WindowBounds"] == "{{82, 11}, {501, 299}}"
    assert store["."]["icvp"]["iconSize"] == 96
    assert store["."]["icvp"]["backgroundType"] == 1
    assert store["."]["lsvp"]["sortColumn"] == "size"
    assert {record.filename for record in store} == {".", "Inserted Item", "Renamed Item"}
    assert store["Inserted Item"]["Iloc"] == (111, 99)
    assert store["Renamed Item"]["Iloc"] == (120, 29)
