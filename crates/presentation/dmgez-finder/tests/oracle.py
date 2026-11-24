"""Check the synthetic Finder layout with the independent Python reader."""

from importlib.metadata import version
from io import BytesIO
import sys

from ds_store import DSStore

assert version("ds_store") == "1.3.4"
assert version("mac_alias") == "2.2.3"

with DSStore.open(BytesIO(sys.stdin.buffer.read())) as store:
    assert len(list(store)) == 6
    assert store["."]["vSrn"] == (b"long", 1)
    assert store["."]["icvl"] == (b"type", b"icnv")
    window = store["."]["bwsp"]
    assert window["WindowBounds"] == "{{92, -140}, {600, 260}}"
    assert window["ShowStatusBar"] and not window["ShowToolbar"]
    assert window["SidebarWidth"] == 125
    icons = store["."]["icvp"]
    assert icons["viewOptionsVersion"] == 1
    assert icons["arrangeBy"] == "none"
    assert icons["backgroundType"] == 1
    assert [icons["backgroundColor" + channel] for channel in ("Red", "Green", "Blue")] == [
        channel / 255 for channel in (34, 68, 102)
    ]
    assert (icons["gridOffsetX"], icons["gridOffsetY"]) == (5, -3)
    assert (icons["scrollPositionX"], icons["scrollPositionY"]) == (2, 3)
    assert (icons["iconSize"], icons["textSize"]) == (96, 15)
    assert not icons["labelOnBottom"]
    assert icons["showIconPreview"] and not icons["showItemInfo"]
    assert store["Applications"]["Iloc"] == (140, 120)
    assert store["Release Notes"]["Iloc"] == (450, 125)
