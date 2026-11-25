#!/usr/bin/env python3
"""Read recursive-copy Finder flags independently from a generated HFS+ catalog."""

import argparse
import os
from pathlib import Path, PurePosixPath
import shutil
from struct import unpack_from
import subprocess
import sys
from tempfile import TemporaryDirectory


HDIUTIL = "/usr/bin/hdiutil"
VOLUME = "Finder Visibility Witness"
IMAGE = "visibility.dmg"
LOGICAL_BYTES = 1 << 20
PAYLOAD = b"synthetic Finder visibility witness\n"
PROJECT = f"""\
schema_version = 1
[image]
name = "{VOLUME}"
output = "{IMAGE}"
format = "udzo"
size = "1MiB"
[metadata]
policy = "report"
[[contents]]
source = "source"
destination = "/Toolkit"
[finder]
hide = ["/Toolkit/Data/Artwork.ext", "/Toolkit/Folder.ext"]
hide_extensions = ["/Toolkit/Data/Artwork.ext", "/Toolkit/Manual.pdf"]
"""

# TN1150 offsets for the partitionless, single-catalog-leaf fixture.
VOLUME_HEADER = 1024
ALLOCATION_BLOCK_SIZE = 40
CATALOG_START_BLOCK = 272 + 16
NODE_DESCRIPTOR_BYTES = 14
NODE_KIND = 8
NODE_RECORD_COUNT = 10
LEAF_KIND = -1
TREE_DEPTH = 0
FIRST_LEAF = 10
LAST_LEAF = 14
NODE_SIZE = 18
PARENT_ID = 2
KEY_NAME_LENGTH = 6
KEY_NAME = 8
RECORD_ID = 8
RECORD_FINDER_FLAGS = 48 + 8
FIELD_BYTES = 2
CATALOG_FOLDER = 1
CATALOG_FILE = 2
ROOT_CNID = 2
INVISIBLE = 0x4000
HIDDEN_EXTENSION = 0x0010

EXPECTED = {
    "Toolkit/Data/Artwork.ext": (CATALOG_FILE, INVISIBLE | HIDDEN_EXTENSION),
    "Toolkit/Folder.ext": (CATALOG_FOLDER, INVISIBLE),
    "Toolkit/Manual.pdf": (CATALOG_FILE, HIDDEN_EXTENSION),
    "Toolkit/Control.txt": (CATALOG_FILE, 0),
}


def u16(data, offset):
    return unpack_from(">H", data, offset)[0]


def u32(data, offset):
    return unpack_from(">I", data, offset)[0]


def catalog_entries(raw):
    header = raw[VOLUME_HEADER:]
    assert header[:2] == b"H+"
    start = u32(header, CATALOG_START_BLOCK) * u32(header, ALLOCATION_BLOCK_SIZE)
    tree = raw[start + NODE_DESCRIPTOR_BYTES:]
    leaf_number = u32(tree, FIRST_LEAF)
    assert leaf_number and u32(tree, LAST_LEAF) == leaf_number and u16(tree, TREE_DEPTH) == 1
    node_size = u16(tree, NODE_SIZE)
    leaf = raw[start + leaf_number * node_size:start + (leaf_number + 1) * node_size]
    assert len(leaf) == node_size and unpack_from(">b", leaf, NODE_KIND)[0] == LEAF_KIND
    entries = {}
    for index in range(u16(leaf, NODE_RECORD_COUNT)):
        key = u16(leaf, len(leaf) - FIELD_BYTES * (index + 1))
        record = key + FIELD_BYTES + u16(leaf, key)
        kind = u16(leaf, record)
        if kind in (CATALOG_FILE, CATALOG_FOLDER):
            name_start = key + KEY_NAME
            name_end = name_start + FIELD_BYTES * u16(leaf, key + KEY_NAME_LENGTH)
            assert name_end == record
            name = leaf[name_start:name_end].decode("utf-16-be")
            parent = u32(leaf, key + PARENT_ID)
            assert (parent, name) not in entries
            entries[parent, name] = (kind, u32(leaf, record + RECORD_ID),
                                     u16(leaf, record + RECORD_FINDER_FLAGS))
    return entries


def check_image(image, raw, seven_zip):
    if sys.platform == "darwin":
        subprocess.run([HDIUTIL, "verify", "-nocache", image], check=True)
    with raw.open("wb") as stream:
        subprocess.run([seven_zip, "x", "-tdmg", "-so", image], stdout=stream, check=True)
    assert raw.stat().st_size == LOGICAL_BYTES
    entries = catalog_entries(raw.read_bytes())
    for path, expected in EXPECTED.items():
        parent = ROOT_CNID
        parts = PurePosixPath(path).parts
        for name in parts[:-1]:
            kind, parent, _ = entries[parent, name]
            assert kind == CATALOG_FOLDER
        kind, _, flags = entries[parent, parts[-1]]
        assert (kind, flags) == expected, (path, kind, flags)
        if kind == CATALOG_FILE:
            selector = f"{VOLUME}/{path}".replace("/", os.sep)
            assert subprocess.check_output([seven_zip, "x", "-thfs", "-so", raw, selector]) == PAYLOAD


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dmgez", type=Path, help="composed CLI with the build feature")
    parser.add_argument("--seven-zip", default="7z")
    args = parser.parse_args()
    seven_zip = shutil.which(args.seven_zip)
    if seven_zip is None:
        parser.error(f"7-Zip executable not found: {args.seven_zip}")
    binary = args.dmgez.resolve(strict=True)
    with TemporaryDirectory(prefix="dmgez-visibility-") as temporary:
        root = Path(temporary)
        source = root / "source"
        (source / "Data").mkdir(parents=True)
        (source / "Folder.ext").mkdir()
        for name in ("Data/Artwork.ext", "Manual.pdf", "Control.txt"):
            (source / name).write_bytes(PAYLOAD)
        (root / "dmg.toml").write_text(PROJECT, encoding="utf-8")
        subprocess.run([binary, "build", "dmg.toml"], cwd=root, check=True)
        check_image(root / IMAGE, root / "volume.hfs", seven_zip)
        for name in ("Data/Artwork.ext", "Manual.pdf", "Control.txt"):
            assert (source / name).read_bytes() == PAYLOAD
    print("Independently verified Finder visibility flags and copied file bytes")


if __name__ == "__main__":
    main()
