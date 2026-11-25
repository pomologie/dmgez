#!/usr/bin/env python3
"""Build, mount, and resolve a Finder background alias with Core Foundation."""

import argparse
import ctypes
from importlib.metadata import version
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys
from tempfile import TemporaryDirectory

from ds_store import DSStore
from mac_alias import Alias

BASE = Path(__file__).resolve().parent
HDIUTIL = "/usr/bin/hdiutil"
FSCK_HFS = "/sbin/fsck_hfs"
WHOLE_DEVICE = re.compile(r"/dev/disk[0-9]+")
CORE_FOUNDATION = "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation"
WITHOUT_UI = 1 << 8
WITHOUT_MOUNTING = 1 << 9
URL_PATH_BUFFER_BYTES = 4096
STORE_NAME = ".DS_Store"
ROOT_RECORD = "."
ICON_VIEW_RECORD = "icvp"
BACKGROUND_TYPE_PROPERTY = "backgroundType"
BACKGROUND_ALIAS_PROPERTY = "backgroundImageAlias"
IMAGE_BACKGROUND_TYPE = 2
BACKGROUND_DIRECTORY = ".background"
NESTED_FOLDER = "Documentation"
IMAGE_NAME = "background.dmg"
SOURCE_NAME = "background.png"
PNG = (BASE / "tests" / "fixtures" / "background-root.png").read_bytes()
REPRODUCIBLE_BUILD = "[build]\nreproducible = true\nsource_date_epoch = 1700000000\n\n"
PROJECT = f"""\
schema_version = 1

[image]
name = "Café Background"
output = "{IMAGE_NAME}"
format = "udzo"
size = "1MiB"

{REPRODUCIBLE_BUILD}[finder]
background = {{ kind = "image", source = "{SOURCE_NAME}" }}

[[finder.folders]]
path = "/{NESTED_FOLDER}"
show_status_bar = true

[[contents]]
type = "directory"
destination = "/{NESTED_FOLDER}"
"""


def resolve_alias(data):
    cf = ctypes.CDLL(CORE_FOUNDATION)
    pointer = ctypes.c_void_p
    boolean = ctypes.c_ubyte
    index = ctypes.c_long
    cf.CFDataCreate.argtypes = (pointer, ctypes.c_char_p, index)
    cf.CFDataCreate.restype = pointer
    cf.CFURLCreateBookmarkDataFromAliasRecord.argtypes = (pointer, pointer)
    cf.CFURLCreateBookmarkDataFromAliasRecord.restype = pointer
    cf.CFURLCreateByResolvingBookmarkData.argtypes = (
        pointer, pointer, ctypes.c_ulong, pointer, pointer, ctypes.POINTER(boolean), pointer,
    )
    cf.CFURLCreateByResolvingBookmarkData.restype = pointer
    cf.CFURLGetFileSystemRepresentation.argtypes = (pointer, boolean, pointer, index)
    cf.CFURLGetFileSystemRepresentation.restype = boolean
    cf.CFRelease.argtypes = (pointer,)
    cf.CFRelease.restype = None

    objects = []

    def retain(value, error):
        assert value, error
        objects.append(value)
        return value

    try:
        alias = retain(cf.CFDataCreate(None, data, len(data)), "could not allocate alias data")
        bookmark = retain(
            cf.CFURLCreateBookmarkDataFromAliasRecord(None, alias),
            "Core Foundation refused the alias record",
        )
        stale = boolean()
        url = retain(
            cf.CFURLCreateByResolvingBookmarkData(
                None, bookmark, WITHOUT_UI | WITHOUT_MOUNTING, None, None,
                ctypes.byref(stale), None,
            ),
            "Core Foundation could not resolve the background on the mounted volume",
        )
        path = ctypes.create_string_buffer(URL_PATH_BUFFER_BYTES)
        assert cf.CFURLGetFileSystemRepresentation(url, True, path, len(path))
        return Path(os.fsdecode(path.value))
    finally:
        for value in reversed(objects):
            cf.CFRelease(value)


def read_alias(store_path):
    with DSStore.open(str(store_path), "r") as store:
        icons = store[ROOT_RECORD][ICON_VIEW_RECORD]
    assert icons[BACKGROUND_TYPE_PROPERTY] == IMAGE_BACKGROUND_TYPE
    return icons[BACKGROUND_ALIAS_PROPERTY]


def verify_hfs(image):
    attached = subprocess.check_output([HDIUTIL, "attach", "-nomount", "-readonly", "-plist", image])
    devices = [
        item["dev-entry"] for item in plistlib.loads(attached)["system-entities"]
        if WHOLE_DEVICE.fullmatch(item.get("dev-entry", ""))
    ]
    try:
        assert len(devices) == 1
        subprocess.run([FSCK_HFS, "-fn", devices[0]], check=True)
    finally:
        for device in devices:
            subprocess.run([HDIUTIL, "detach", device], check=True)


def qualify_mount(image, mount):
    subprocess.run([HDIUTIL, "verify", image], check=True)
    verify_hfs(image)
    mount.mkdir()
    subprocess.run([
        HDIUTIL, "attach", "-readonly", "-nobrowse", "-noautoopen", "-mount", "required",
        "-mountpoint", mount, image,
    ], check=True)
    try:
        encoded = read_alias(mount / STORE_NAME)
        assert read_alias(mount / NESTED_FOLDER / STORE_NAME) == encoded
        alias = Alias.from_bytes(encoded)
        target = Path(alias.target.posix_path)
        assert target.parts[0:2] == ("/", BACKGROUND_DIRECTORY) and len(target.parts) == 3
        background = mount.joinpath(*target.parts[1:])
        assert resolve_alias(encoded).resolve(strict=True) == background.resolve(strict=True)
        assert background.read_bytes() == PNG
        assert background.stat().st_ino == alias.target.cnid
        assert background.parent.stat().st_ino == alias.target.folder_cnid
    finally:
        subprocess.run([HDIUTIL, "detach", mount], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dmgez", type=Path, required=True)
    parser.add_argument("--hfsplus", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("the native Finder background check requires macOS")
    assert version("ds_store") == "1.3.4"
    assert version("mac_alias") == "2.2.3"
    with TemporaryDirectory(prefix="dmgez-finder-background-") as temporary:
        for profile, source in (
            ("reproducible", PROJECT),
            ("default-dates", PROJECT.replace(REPRODUCIBLE_BUILD, "")),
        ):
            root = Path(temporary).resolve() / profile
            root.mkdir()
            project = root / "project.toml"
            project.write_text(source, encoding="utf-8")
            (root / SOURCE_NAME).write_bytes(PNG)
            runner = [
                sys.executable, BASE / "run.py", "--dmgez", args.dmgez, "--hfsplus", args.hfsplus,
                "--project", project,
            ]
            dry_run = subprocess.check_output([*runner, "--dry-run"], text=True)
            assert "finder_assets=3" in dry_run.splitlines()
            assert not (root / IMAGE_NAME).exists()
            subprocess.run(runner, check=True)
            qualify_mount(root / IMAGE_NAME, root / "mount")
    print("Core Foundation resolved both mounted Finder backgrounds with matching catalog IDs")


if __name__ == "__main__":
    main()
