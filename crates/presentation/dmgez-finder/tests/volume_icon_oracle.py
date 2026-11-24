"""Recreate the synthetic ICNS fixture and optionally check its HFS+ flag."""

import argparse
from hashlib import sha256
from pathlib import Path
import plistlib
import re
from struct import pack, unpack_from
import subprocess
from tempfile import TemporaryDirectory
from zlib import compress, crc32


FIXTURE = Path(__file__).parent / "fixtures" / "apple-iconutil.icns"
ICONUTIL = "/usr/bin/iconutil"
HDIUTIL = "/usr/bin/hdiutil"
XCRUN = "/usr/bin/xcrun"
XATTR = "/usr/bin/xattr"
ICON_NAME = ".VolumeIcon.icns"
FINDER_INFO = "com.apple.FinderInfo"
CUSTOM_ICON_BIT = 0x0400
FINDER_FLAGS_OFFSET = 8
FINDER_INFO_BYTES = 32
ICONSET_POINT_SIZES = (16, 32, 128, 256, 512)
ICONSET_SCALES = (1, 2)
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
PNG_DEPTH = 8
PNG_RGBA = 6
COLORS = ((22, 127, 116, 255), (251, 190, 66, 255))

# Offsets from TN1150 for the native, unpartitioned, single-leaf test volume.
VOLUME_HEADER_OFFSET = 1024
ALLOCATION_BLOCK_SIZE = 40
VOLUME_FINDER_INFO = 80
CATALOG_FIRST_EXTENT = 272 + 16
BTREE_NODE_DESCRIPTOR_BYTES = 14
BTREE_DEPTH = 0
BTREE_FIRST_LEAF = 10
BTREE_LAST_LEAF = 14
BTREE_NODE_SIZE = 18
NODE_KIND = 8
NODE_RECORD_COUNT = 10
LEAF_KIND = -1
CATALOG_FOLDER = 1
ROOT_CNID = 2
CATALOG_FOLDER_ID = 8
CATALOG_FOLDER_FINDER_INFO = 48


def run(*args):
    return subprocess.check_output(args)


def png_chunk(tag, payload):
    return pack(">I", len(payload)) + tag + payload + pack(">I", crc32(tag + payload))


def write_png(path, side):
    rows = bytearray()
    for y in range(side):
        rows.append(0)
        for x in range(side):
            rows.extend(COLORS[int(x + y >= side)])
    header = pack(">IIBBBBB", side, side, PNG_DEPTH, PNG_RGBA, 0, 0, 0)
    path.write_bytes(
        PNG_SIGNATURE
        + png_chunk(b"IHDR", header)
        + png_chunk(b"IDAT", compress(rows, 9))
        + png_chunk(b"IEND", b"")
    )


def build_asset(probe):
    iconset = probe / "synthetic.iconset"
    iconset.mkdir()
    for points in ICONSET_POINT_SIZES:
        for scale in ICONSET_SCALES:
            density = "@2x" if scale == 2 else ""
            filename = f"icon_{points}x{points}{density}.png"
            write_png(iconset / filename, points * scale)

    asset = probe / "synthetic.icns"
    run(ICONUTIL, "-c", "icns", "-o", asset, iconset)
    roundtrip = probe / "decoded.iconset"
    run(ICONUTIL, "-c", "iconset", "-o", roundtrip, asset)
    inputs = {entry.name: unpack_from(">II", entry.read_bytes(), 16) for entry in iconset.iterdir()}
    outputs = {entry.name: unpack_from(">II", entry.read_bytes(), 16) for entry in roundtrip.iterdir()}
    if inputs != outputs:
        raise RuntimeError("Apple did not decode all representations at the expected dimensions")
    return asset


def check_native_volume(probe, asset):
    image = probe / "native-hfs.dmg"
    run(
        HDIUTIL, "create", "-size", "8m", "-layout", "NONE", "-fs", "HFS+",
        "-volname", "dmgez Icon Oracle", image,
    )
    mount = probe / "mount"
    mount.mkdir()

    attachment = plistlib.loads(run(
        HDIUTIL, "attach", "-plist", "-nobrowse", "-noautoopen", "-owners", "off",
        "-mountpoint", mount, "-mount", "required", image,
    ))
    entities = attachment["system-entities"]
    devices = {
        entry["dev-entry"]
        for entry in entities
        if re.fullmatch(r"/dev/disk[1-9][0-9]*", entry.get("dev-entry", ""))
    }
    try:
        if len(devices) != 1:
            raise RuntimeError("expected one whole-image device for the disposable volume")
        if mount.resolve() not in {
            Path(entry["mount-point"]).resolve() for entry in entities if "mount-point" in entry
        }:
            raise RuntimeError("disposable volume did not attach at the requested path")
        if run(XCRUN, "GetFileInfo", "-aC", mount).strip() != b"0":
            raise RuntimeError("fresh filesystem already has the custom-icon flag")
        (mount / ICON_NAME).write_bytes(asset.read_bytes())
        run(XCRUN, "SetFile", "-a", "C", mount)
        if run(XCRUN, "GetFileInfo", "-aC", mount).strip() != b"1":
            raise RuntimeError("Apple's custom-icon flag did not read back")
        finder_info = bytes.fromhex(run(XATTR, "-px", FINDER_INFO, mount).decode("ascii"))
        if len(finder_info) != FINDER_INFO_BYTES or not read_flags(finder_info, 0) & CUSTOM_ICON_BIT:
            raise RuntimeError("Apple did not set the documented root FinderInfo bit")
        if (mount / ICON_NAME).read_bytes() != asset.read_bytes():
            raise RuntimeError("the on-volume ICNS bytes changed")
    finally:
        for device in devices:
            run(HDIUTIL, "detach", device)

    raw = probe / "native-hfs"
    run(HDIUTIL, "convert", image, "-format", "UDTO", "-o", raw)
    verify_catalog_flag(raw.with_suffix(".cdr").read_bytes())


def u16(data, offset):
    return unpack_from(">H", data, offset)[0]


def u32(data, offset):
    return unpack_from(">I", data, offset)[0]


def read_flags(data, offset):
    return u16(data, offset + FINDER_FLAGS_OFFSET)


def verify_catalog_flag(raw):
    header = raw[VOLUME_HEADER_OFFSET:]
    if header[:2] != b"H+" or read_flags(header, VOLUME_FINDER_INFO) & CUSTOM_ICON_BIT:
        raise RuntimeError("unexpected signature or use of the volume-header Finder fields")
    block_size = u32(header, ALLOCATION_BLOCK_SIZE)
    catalog_start = u32(header, CATALOG_FIRST_EXTENT) * block_size
    tree_header = raw[catalog_start + BTREE_NODE_DESCRIPTOR_BYTES:]
    first_leaf = u32(tree_header, BTREE_FIRST_LEAF)
    last_leaf = u32(tree_header, BTREE_LAST_LEAF)
    if u16(tree_header, BTREE_DEPTH) != 1 or first_leaf != last_leaf or first_leaf == 0:
        raise RuntimeError("expected one catalog leaf in the native fixture")
    node_size = u16(tree_header, BTREE_NODE_SIZE)
    leaf_start = catalog_start + first_leaf * node_size
    leaf = raw[leaf_start:leaf_start + node_size]
    if unpack_from(">b", leaf, NODE_KIND)[0] != LEAF_KIND:
        raise RuntimeError("catalog node is not a leaf")
    for index in range(u16(leaf, NODE_RECORD_COUNT)):
        offset = u16(leaf, len(leaf) - 2 * (index + 1))
        record = offset + 2 + u16(leaf, offset)
        if u16(leaf, record) == CATALOG_FOLDER and u32(leaf, record + CATALOG_FOLDER_ID) == ROOT_CNID:
            if not read_flags(leaf, record + CATALOG_FOLDER_FINDER_INFO) & CUSTOM_ICON_BIT:
                raise RuntimeError("root catalog FinderInfo lacks the documented flag")
            return
    raise RuntimeError("root catalog record not found")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check-volume", action="store_true")
    args = parser.parse_args()
    with TemporaryDirectory(prefix="dmgez-volume-icon-") as probe_dir:
        probe = Path(probe_dir)
        asset = build_asset(probe)
        if asset.read_bytes() != FIXTURE.read_bytes():
            raise RuntimeError("recreated ICNS differs from the checked-in macOS 14.7.2 fixture")
        print(f"Apple decoded all ten icon representations. SHA-256 {sha256(asset.read_bytes()).hexdigest()}")
        if args.check_volume:
            check_native_volume(probe, asset)
            print("Apple preserved the ICNS, and SetFile set the custom-icon bit in the HFS+ root catalog record.")


if __name__ == "__main__":
    main()
