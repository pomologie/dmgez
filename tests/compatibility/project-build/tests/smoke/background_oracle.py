"""Compare independently decoded Finder aliases with actual HFS+ catalog data."""

from datetime import timedelta
from importlib.metadata import version
from io import BytesIO
import sys

from ds_store import DSStore
from mac_alias import Alias
from mac_alias.alias import ALIAS_KIND_FILE, mac_epoch


assert version("ds_store") == "1.3.4"
assert version("mac_alias") == "2.2.3"

kind, *target = sys.argv[1:]
with DSStore.open(BytesIO(sys.stdin.buffer.read())) as store:
    assert store["."]["icvl"] == (b"type", b"icnv")
    icons = store["."]["icvp"]
    if kind == "root":
        assert store["Inherited"]["Iloc"] == (50, 70)
    if kind == "color":
        assert icons["backgroundType"] == 1
        assert "backgroundImageAlias" not in icons
    else:
        name, volume_date, filename, cnid, parent, date, path = target
        assert icons["backgroundType"] == 2
        assert isinstance(icons["backgroundImageAlias"], bytes)
        alias = Alias.from_bytes(icons["backgroundImageAlias"])
        assert alias.version == 2
        assert alias.volume.name == name
        assert alias.volume.fs_type == b"H+"
        assert alias.volume.creation_date == mac_epoch + timedelta(seconds=int(volume_date))
        assert alias.target.kind == ALIAS_KIND_FILE
        assert alias.target.filename == filename
        assert alias.target.cnid == int(cnid)
        assert alias.target.folder_cnid == int(parent)
        assert alias.target.creation_date == mac_epoch + timedelta(seconds=int(date))
        assert alias.target.posix_path == path
