#!/usr/bin/env python3
"""Build and check a mounted built-in background and badged volume icon."""

import argparse
from importlib.metadata import version
from pathlib import Path
import runpy
from struct import Struct
import subprocess
import sys
from tempfile import TemporaryDirectory

from check_finder_background import IMAGE_NAME, NESTED_FOLDER, TIFF_ASSET, qualify_mount
from check_finder_volume_icon import (
    FIXTURE, ICNS_HEADER, ICNS_MAGIC, ICON_ASSET, ICONUTIL, ISOLATED_ICONSET, OUTPUT_ICONSET,
    PIXEL_CHECK, XCRUN, isolate_elements,
)


BASE = Path(__file__).resolve().parent
BADGE_SOURCE = "badge.png"
ICNS_BADGE_SOURCE = "badge.icns"
ICNS_BADGE_IMAGE = "icns-badge.dmg"
ICNS_BADGE_SLOT = "16@2x"
BADGE_FIXTURE = FIXTURE.with_name("badge_icon_fixture.py")
PNG_DIMENSIONS = Struct(">II")
PNG_DIMENSION_OFFSET = 16
PROJECT = f"""\
schema_version = 1

[image]
name = "Café Installer Artwork"
output = "{IMAGE_NAME}"
format = "udzo"
size = "8MiB"

[build]
reproducible = true
source_date_epoch = 1700000000

[finder]
background = {{ kind = "template", name = "builtin-arrow" }}

[[finder.folders]]
path = "/{NESTED_FOLDER}"
show_status_bar = true

[[contents]]
type = "directory"
destination = "/{NESTED_FOLDER}"

[volume_icon]
badge = "{BADGE_SOURCE}"
"""


def qualify_artwork(background, root, slots, verifier):
    mount = background.parent.parent
    flag = subprocess.check_output([XCRUN, "GetFileInfo", "-aC", mount]).strip()
    if flag != b"1":
        raise RuntimeError("Apple did not read the custom-icon bit on the mounted root")
    icon = mount / ICON_ASSET
    decoded = root / OUTPUT_ICONSET
    subprocess.run([ICONUTIL, "-c", "iconset", "-o", decoded, icon], check=True)
    expected = {slot.filename: (slot.pixels, slot.pixels) for slot in slots}
    actual = {
        path.name: PNG_DIMENSIONS.unpack_from(path.read_bytes(), PNG_DIMENSION_OFFSET)
        for path in decoded.iterdir() if path.suffix == ".png"
    }
    if actual != expected:
        raise RuntimeError("Apple did not extract the ten expected badged icon sizes")
    isolated = root / ISOLATED_ICONSET
    isolate_elements(icon, slots, isolated)
    subprocess.run([verifier, "--artwork", background, isolated], check=True)


def verify_icns_source(png_build, root, runner, project, slots):
    kind = next(slot.kind for slot in slots if slot.key == ICNS_BADGE_SLOT)
    png = (root / BADGE_SOURCE).read_bytes()
    element = ICNS_HEADER.pack(kind, ICNS_HEADER.size + len(png)) + png
    (root / ICNS_BADGE_SOURCE).write_bytes(
        ICNS_HEADER.pack(ICNS_MAGIC, ICNS_HEADER.size + len(element)) + element
    )
    project.write_text(
        PROJECT.replace(BADGE_SOURCE, ICNS_BADGE_SOURCE).replace(IMAGE_NAME, ICNS_BADGE_IMAGE),
        encoding="utf-8",
    )
    subprocess.run(runner, check=True)
    if png_build.read_bytes() != (root / ICNS_BADGE_IMAGE).read_bytes():
        raise RuntimeError("equivalent PNG and ICNS badges produced different images")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dmgez", type=Path, required=True)
    parser.add_argument("--hfsplus", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("native Finder artwork verification requires macOS")
    assert version("ds_store") == "1.3.4"
    assert version("mac_alias") == "2.2.3"
    fixture = runpy.run_path(str(FIXTURE))
    with TemporaryDirectory(prefix="dmgez-finder-artwork-") as temporary:
        root = Path(temporary).resolve()
        project = root / "project.toml"
        project.write_text(PROJECT, encoding="utf-8")
        subprocess.run([sys.executable, "-B", BADGE_FIXTURE, "--write", root / BADGE_SOURCE], check=True)
        runner = [
            sys.executable, BASE / "run.py", "--dmgez", args.dmgez,
            "--hfsplus", args.hfsplus, "--badge-icons", "--project", project,
        ]
        image = root / IMAGE_NAME
        dry_run = subprocess.check_output([*runner, "--dry-run"], text=True)
        if image.exists() or "finder_assets=4" not in dry_run.splitlines():
            raise RuntimeError("dry-run failed to reserve the artwork and Finder stores")
        subprocess.run(runner, check=True)
        verify_icns_source(image, root, runner, project, fixture["SLOTS"])
        verifier = root / "verify-artwork"
        subprocess.run([
            XCRUN, "swiftc", "-module-cache-path", root / "swift-cache",
            PIXEL_CHECK, "-o", verifier,
        ], check=True)
        qualify_mount(
            image, root / "mount", TIFF_ASSET,
            lambda asset: qualify_artwork(asset, root, fixture["SLOTS"], verifier),
        )
    print("Core Foundation resolved the built-in background. Apple read the badged volume icon.")
    print("Equivalent PNG and ICNS badge sources produced identical images.")


if __name__ == "__main__":
    main()
