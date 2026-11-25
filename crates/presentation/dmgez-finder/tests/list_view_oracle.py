"""Check both list-view record representations using the independent reader."""

from importlib.metadata import version
from io import BytesIO
import plistlib
import sys

from ds_store import DSStore

assert version("ds_store") == "1.3.4"
assert version("mac_alias") == "2.2.3"

with DSStore.open(BytesIO(sys.stdin.buffer.read()), "r") as store:
    assert store["."]["icvl"] == (b"type", b"Nlsv")
    keyed = store["."]["lsvp"]
    wire_type, ordered_blob = store["."]["lsvC"]
    assert wire_type == b"blob"
    ordered = plistlib.loads(bytes(ordered_blob))
    assert {key: value for key, value in keyed.items() if key != "columns"} == {
        key: value for key, value in ordered.items() if key != "columns"
    }
    assert keyed["sortColumn"] == "size"
    assert keyed["scrollPositionX"] == 2.5
    assert keyed["scrollPositionY"] == 3.5
    assert [column["identifier"] for column in ordered["columns"][:3]] == [
        "name", "dateAdded", "size"
    ]
    for position, column in enumerate(ordered["columns"]):
        legacy = keyed["columns"][column["identifier"]]
        assert legacy["index"] == position
        assert type(column["width"]) is int and legacy["width"] == column["width"]
        assert legacy["visible"] == column["visible"]
        assert legacy["ascending"] == column["ascending"]
    hidden = keyed["columns"]["dateLastOpened"]
    assert not hidden["visible"] and hidden["ascending"] and hidden["width"] == 211
