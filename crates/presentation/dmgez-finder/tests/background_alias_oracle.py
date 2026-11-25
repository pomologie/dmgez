"""Check the image-background property list and synthetic file identity."""

from datetime import timedelta
from importlib.metadata import version
from io import BytesIO
import sys

from ds_store import DSStore
from mac_alias import Alias, TargetInfo, VolumeInfo
from mac_alias.alias import ALIAS_FIXED_DISK, ALIAS_KIND_FILE, mac_epoch

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
    assert alias.target.filename == "desktop.png"
    assert alias.target.cnid == 20
    assert alias.target.folder_cnid == 16
    assert alias.target.creation_date == mac_epoch + timedelta(seconds=3_801_000_123)
    assert alias.target.posix_path == "/.background/desktop.png"
    assert alias.target.folder_name == ".background"
    assert alias.target.cnid_path == (16,)
    assert alias.target.carbon_path == b"Example Installer:.background:\0desktop.png"

    volume = VolumeInfo(
        "Example Installer", mac_epoch + timedelta(seconds=3_801_000_000),
        b"H+", ALIAS_FIXED_DISK, 0, b"\0\0",
    )
    target = TargetInfo(
        ALIAS_KIND_FILE, "desktop.png", 16, 20,
        mac_epoch + timedelta(seconds=3_801_000_123), b"\0" * 4, b"\0" * 4,
        folder_name=".background", cnid_path=(16,),
        carbon_path=b"Example Installer:.background:\0desktop.png",
        posix_path="/.background/desktop.png",
    )
    assert alias_bytes == Alias(volume=volume, target=target).to_bytes()
