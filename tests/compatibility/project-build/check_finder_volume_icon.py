#!/usr/bin/env python3
"""Verify a generated ICNS from ten PNGs on a mounted HFS+ project image."""

import argparse
from pathlib import Path
import runpy
from struct import Struct
import subprocess
import sys
from tempfile import TemporaryDirectory


BASE = Path(__file__).resolve().parent
FIXTURE = BASE / "tests" / "smoke" / "volume_icon_fixture.py"
PIXEL_CHECK = BASE / "check_volume_icon_pixels.swift"
HDIUTIL = "/usr/bin/hdiutil"
ICONUTIL = "/usr/bin/iconutil"
XCRUN = "/usr/bin/xcrun"
ICON_ASSET = ".VolumeIcon.icns"
IMAGE_NAME = "volume-icon.dmg"
INPUT_ICONSET = "input.iconset"
OUTPUT_ICONSET = "decoded.iconset"
ISOLATED_ICONSET = "isolated-elements"
ICNS_HEADER = Struct(">4sI")
ICNS_MAGIC = b"icns"
PROJECT = f"""\
schema_version = 1

[image]
name = "Café Volume Icon"
output = "{IMAGE_NAME}"
format = "udzo"
size = "1MiB"

[build]
reproducible = true
source_date_epoch = 1700000000

[volume_icon.images]
"""


def isolate_elements(asset, slots, directory):
    """Give ImageIO one representation at a time to avoid same-size selection."""
    encoded = asset.read_bytes()
    if ICNS_HEADER.unpack_from(encoded) != (ICNS_MAGIC, len(encoded)):
        raise RuntimeError("unexpected ICNS header")
    payloads = {}
    offset = ICNS_HEADER.size
    while offset < len(encoded):
        kind, size = ICNS_HEADER.unpack_from(encoded, offset)
        end = offset + size
        if size <= ICNS_HEADER.size or end > len(encoded) or kind in payloads:
            raise RuntimeError("unexpected generated ICNS element framing")
        payloads[kind] = encoded[offset:end]
        offset = end
    directory.mkdir()
    for slot in slots:
        element = payloads[slot.kind]
        output = directory / Path(slot.filename).with_suffix(".icns")
        output.write_bytes(ICNS_HEADER.pack(ICNS_MAGIC, ICNS_HEADER.size + len(element)) + element)


def check_mounted_icon(image, root, inputs, slots, verifier):
    mount = root / "mount"
    mount.mkdir()
    subprocess.run([HDIUTIL, "verify", image], check=True)
    subprocess.run([
        HDIUTIL, "attach", "-readonly", "-nobrowse", "-noautoopen", "-mount", "required",
        "-mountpoint", mount, image,
    ], check=True)
    try:
        flag = subprocess.check_output([XCRUN, "GetFileInfo", "-aC", mount]).strip()
        if flag != b"1":
            raise RuntimeError("Apple did not read the custom-icon bit on the mounted root")
        asset = mount / ICON_ASSET
        decoded = root / OUTPUT_ICONSET
        isolated = root / ISOLATED_ICONSET
        subprocess.run([ICONUTIL, "-c", "iconset", "-o", decoded, asset], check=True)
        isolate_elements(asset, slots, isolated)
        subprocess.run([verifier, inputs, decoded, isolated], check=True)
    finally:
        subprocess.run([HDIUTIL, "detach", mount], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dmgez", type=Path, required=True)
    parser.add_argument("--hfsplus", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("the native Finder volume icon check requires macOS")

    fixture = runpy.run_path(str(FIXTURE))
    slots = fixture["SLOTS"]
    with TemporaryDirectory(prefix="dmgez-generated-volume-icon-") as temporary:
        root = Path(temporary).resolve()
        inputs = root / INPUT_ICONSET
        inputs.mkdir()
        fixture["write_pngs"](inputs)
        entries = "".join(
            f'"{slot.key}" = "{INPUT_ICONSET}/{slot.filename}"\n' for slot in slots
        )
        project = root / "project.toml"
        project.write_text(PROJECT + entries, encoding="utf-8")
        runner = [
            sys.executable, BASE / "run.py", "--dmgez", args.dmgez, "--hfsplus", args.hfsplus,
            "--volume-icon", "--project", project,
        ]
        image = root / IMAGE_NAME
        dry_run = subprocess.check_output([*runner, "--dry-run"], text=True)
        if image.exists() or "finder_assets=2" not in dry_run.splitlines():
            raise RuntimeError("dry-run failed to reserve the icon and store without publishing")
        subprocess.run(runner, check=True)

        verifier = root / "verify-icon-pixels"
        subprocess.run([
            XCRUN, "swiftc", "-module-cache-path", root / "swift-cache",
            PIXEL_CHECK, "-o", verifier,
        ], check=True)
        check_mounted_icon(image, root, inputs, slots, verifier)
    print("Apple verified the image and read its custom volume icon flag.")


if __name__ == "__main__":
    main()
