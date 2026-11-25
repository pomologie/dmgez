"""Check the image-background property list and synthetic file identity."""

from datetime import timedelta
from importlib.metadata import version
from io import BytesIO
import sys

from ds_store import DSStore
from mac_alias import Alias
from mac_alias.alias import ALIAS_KIND_FILE, mac_epoch

assert version("ds_store") == "1.3.4"
assert version("mac_alias") == "2.2.3"

with DSStore.open(BytesIO(sys.stdin.buffer.read())) as store:
    assert store["."]["icvl"] == (b"type", b"icnv")
    view = store["."]["icvp"]
    assert view["backgroundType"] == 2
    assert view["iconSize"] == 128.0
    alias_bytes = view["backgroundImageAlias"]
    assert isinstance(alias_bytes, bytes)
    alias = Alias.from_bytes(alias_bytes)
    assert alias.version == 2
    assert alias.volume.name == "Example Installer"
    assert alias.volume.fs_type == b"H+"
    assert alias.volume.creation_date == mac_epoch + timedelta(seconds=3_801_000_000)
    assert alias.volume.posix_path is None
    assert alias.target.kind == ALIAS_KIND_FILE
    assert alias.target.filename == ".background.png"
    assert alias.target.cnid == 20
    assert alias.target.folder_cnid == 2
    assert alias.target.creation_date == mac_epoch + timedelta(seconds=3_801_000_123)
    assert alias.target.posix_path == "/.background.png"
