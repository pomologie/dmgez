"""Emit a synthetic store with the independent Python writer."""

from importlib.metadata import version
from pathlib import Path
import sys
from tempfile import TemporaryDirectory

from ds_store import DSStore

assert version("ds_store") == "1.3.4"
assert version("mac_alias") == "2.2.3"

with TemporaryDirectory(prefix="dmgez-finder-oracle-") as temporary:
    path = Path(temporary) / ".DS_Store"
    with DSStore.open(str(path), "w+") as store:
        store["."]["vSrn"] = ("long", 1)
        store["."]["icvl"] = ("type", b"icnv")
        store["."]["bwsp"] = {
            "WindowBounds": "{{75, -50}, {620, 270}}",
            "ShowStatusBar": True,
            "ShowToolbar": False,
            "PreviewPaneVisibility": False,
        }
        store["."]["icvp"] = {
            "arrangeBy": "none",
            "backgroundType": 1,
            "backgroundColorRed": 34 / 255,
            "backgroundColorGreen": 68 / 255,
            "backgroundColorBlue": 102 / 255,
            "scrollPositionX": 2,
            "scrollPositionY": 3,
            "showIconPreview": True,
            "showItemInfo": False,
        }
        store["Example"]["Iloc"] = (150, 175)
    sys.stdout.buffer.write(path.read_bytes())
