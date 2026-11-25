#!/usr/bin/env python3
"""Verify per-file and per-folder custom icons in a dmgez HFS+ image on macOS."""

import argparse
from pathlib import Path
import shutil
from struct import Struct
import subprocess
import sys
from tempfile import TemporaryDirectory

from check_finder_volume_icon import HDIUTIL, XCRUN


BASE = Path(__file__).resolve().parent
PIXEL_CHECK = BASE / "check_item_icon_pixels.swift"
XATTR = "/usr/bin/xattr"
ICON_FIXTURE = Path("crates/presentation/dmgez-finder/tests/fixtures/apple-iconutil.icns")
ICON_SOURCE = "item-icon.icns"
IMAGE_NAME = "item-icons.dmg"
FILE_NAME = "Document.txt"
FOLDER_NAME = "Documentation"
CONTROL_FILE_NAME = "Generic.txt"
CONTROL_FOLDER_NAME = "Generic Folder"
ICON_CHILD_NAME = "Icon\r"
FILE_BYTES = b"Owned custom icon fixture\n"
FINDER_INFO = "com.apple.FinderInfo"
FINDER_INFO_BYTES = 32
FLAGS_OFFSET = 8
FLAGS_ENCODING = Struct(">H")
HAS_CUSTOM_ICON = 0x0400
IS_INVISIBLE = 0x4000
RESOURCE_FORK_PATH = "..namedfork/rsrc"
RESOURCE_HEADER = Struct(">IIII")
RESOURCE_SIZE = Struct(">I")
MAP_LISTS = Struct(">HHh")
MAP_TYPE_AND_REFERENCE = Struct(">4sHHhhII")
RESOURCE_DATA_OFFSET = 256
RESOURCE_MAP_BYTES = 50
RESOURCE_MAP_RESERVED_BYTES = 8
TYPE_LIST_OFFSET = 28
FIRST_REFERENCE_OFFSET = 10
RESOURCE_TYPE = b"icns"
CUSTOM_ICON_RESOURCE_ID = -16455
UNNAMED_RESOURCE = -1
PROJECT = f"""\
schema_version = 1

[image]
name = "dmgez Item Icon Witness"
output = "{IMAGE_NAME}"
format = "udzo"
size = "1MiB"

[build]
reproducible = true
source_date_epoch = 946684800

[metadata]
preserve_timestamps = false
preserve_extended_attributes = false

[[contents]]
source = "{FILE_NAME}"
destination = "/{FILE_NAME}"
custom_icon = "{ICON_SOURCE}"

[[contents]]
type = "directory"
destination = "/{FOLDER_NAME}"
custom_icon = "{ICON_SOURCE}"

[[contents]]
source = "{CONTROL_FILE_NAME}"
destination = "/{CONTROL_FILE_NAME}"

[[contents]]
type = "directory"
destination = "/{CONTROL_FOLDER_NAME}"
"""


def expected_fork(icns):
    """Build the single-resource layout independently from the Apple witness."""
    data = RESOURCE_SIZE.pack(len(icns)) + icns
    header = RESOURCE_HEADER.pack(
        RESOURCE_DATA_OFFSET, RESOURCE_DATA_OFFSET + len(data), len(data), RESOURCE_MAP_BYTES,
    )
    # The reference handle is runtime state. A zero handle is accepted on a native mount.
    mapping = (
        header
        + bytes(RESOURCE_MAP_RESERVED_BYTES)
        + MAP_LISTS.pack(TYPE_LIST_OFFSET, RESOURCE_MAP_BYTES, 0)
        + MAP_TYPE_AND_REFERENCE.pack(
            RESOURCE_TYPE, 0, FIRST_REFERENCE_OFFSET, CUSTOM_ICON_RESOURCE_ID, UNNAMED_RESOURCE,
            0, 0,
        )
    )
    assert len(mapping) == RESOURCE_MAP_BYTES
    return header + bytes(RESOURCE_DATA_OFFSET - len(header)) + data + mapping


def finder_info(path):
    result = subprocess.check_output([XATTR, "-px", FINDER_INFO, path])
    return bytes.fromhex(result.decode("ascii"))


def expected_finder_info(flags):
    expected = bytearray(FINDER_INFO_BYTES)
    FLAGS_ENCODING.pack_into(expected, FLAGS_OFFSET, flags)
    return expected


def require_flag(path, letter):
    result = subprocess.check_output([XCRUN, "GetFileInfo", "-a" + letter, path])
    if result.strip() != b"1":
        raise RuntimeError(f"Apple did not read Finder flag {letter} for {path.name!r}")


def check_mounted_icons(image, root, fixture, verifier):
    mount = root / "mount"
    mount.mkdir()
    subprocess.run([HDIUTIL, "verify", image], check=True)
    subprocess.run([
        HDIUTIL, "attach", "-readonly", "-nobrowse", "-noautoopen", "-mount", "required",
        "-mountpoint", mount, image,
    ], check=True)
    try:
        file = mount / FILE_NAME
        folder = mount / FOLDER_NAME
        child = folder / ICON_CHILD_NAME
        if file.read_bytes() != FILE_BYTES or child.read_bytes():
            raise RuntimeError("mounted data fork differs from the expected bytes")
        if [entry.name for entry in folder.iterdir()] != [ICON_CHILD_NAME]:
            raise RuntimeError("custom folder icon did not produce exactly one native icon child")
        for target in (file, folder):
            if finder_info(target) != expected_finder_info(HAS_CUSTOM_ICON):
                raise RuntimeError(f"unexpected FinderInfo for {target.name!r}")
            require_flag(target, "C")
        if finder_info(child) != expected_finder_info(IS_INVISIBLE):
            raise RuntimeError("the folder icon child lacks its exact invisible FinderInfo")
        require_flag(child, "V")
        fork = expected_fork(fixture.read_bytes())
        for target in (file, child):
            if (target / RESOURCE_FORK_PATH).read_bytes() != fork:
                raise RuntimeError(f"resource fork differs for {target.name!r}")
        subprocess.run([
            verifier, fixture, file, folder, mount / CONTROL_FILE_NAME, mount / CONTROL_FOLDER_NAME,
        ], check=True)
    finally:
        subprocess.run([HDIUTIL, "detach", mount], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dmgez", type=Path, required=True)
    parser.add_argument("--hfsplus", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("the native Finder item icon check requires macOS")

    with TemporaryDirectory(prefix="dmgez-item-icon-") as temporary:
        root = Path(temporary).resolve()
        fixture = root / ICON_SOURCE
        shutil.copyfile(args.dmgez / ICON_FIXTURE, fixture)
        for filename in (FILE_NAME, CONTROL_FILE_NAME):
            (root / filename).write_bytes(FILE_BYTES)
        project = root / "project.toml"
        project.write_text(PROJECT, encoding="utf-8")
        runner = [
            sys.executable, BASE / "run.py", "--dmgez", args.dmgez, "--hfsplus", args.hfsplus,
            "--volume-icon", "--project", project,
        ]
        image = root / IMAGE_NAME
        subprocess.run([*runner, "--dry-run"], check=True)
        if image.exists():
            raise RuntimeError("dry-run published an output")
        subprocess.run(runner, check=True)
        verifier = root / "verify-item-icon-pixels"
        subprocess.run([
            XCRUN, "swiftc", "-module-cache-path", root / "swift-cache",
            PIXEL_CHECK, "-o", verifier,
        ], check=True)
        check_mounted_icons(image, root, fixture, verifier)
        if any((root / filename).read_bytes() != FILE_BYTES
               for filename in (FILE_NAME, CONTROL_FILE_NAME)):
            raise RuntimeError("builder modified the host source data")


if __name__ == "__main__":
    main()
