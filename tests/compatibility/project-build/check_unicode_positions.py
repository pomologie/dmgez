#!/usr/bin/env python3
"""Compare Finder position keys with names decoded independently from HFS+."""

import argparse
from importlib.metadata import version
from io import BytesIO
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory

from ds_store import DSStore


BASE = Path(__file__).resolve().parent
BACKGROUND = BASE.parents[1] / "fixtures" / "source" / "actual-preview-udzo" / "background.png"
VOLUME = "Unicode Finder"
PAYLOAD = b"positioned payload\0\xff\n"
ORIGINAL_NAME = "Cafe\u0301 🚀"
RENAMED_NAME = "Re\u0301sume\u0301 🎉"
NESTED_NAME = "A\u030angstro\u0308m"
PROJECT = """\
schema_version = 1
[image]
name = "Unicode Finder"
output = "original.dmg"
format = "udzo"
size = "4MiB"
[metadata]
policy = "report"
preserve_timestamps = false
preserve_finder_info = false
[[contents]]
source = "payload.bin"
destination = "/Café 🚀"
position = [17, 23]
[[contents]]
source = "payload.bin"
destination = "/Guide/Ångström"
[finder]
background = { kind = "image", source = "background.png" }
[[finder.folders]]
path = "/Guide"
background = { kind = "none" }
[finder.folders.icon_view.positions]
"Ångström" = [31, 47]
"""
EDITS = """\
schema_version = 1
[[edits]]
operation = "modify"
target = "/café 🚀"
destination = "/Résumé 🎉"
position = [29, 53]
[[edits]]
operation = "modify"
target = "/guide/ångström"
position = [59, 61]
"""


def entries(seven_zip, raw):
    output = subprocess.check_output([seven_zip, "l", "-thfs", "-slt", raw], encoding="utf-8")
    return {line.removeprefix("Path = ") for line in output.splitlines() if line.startswith("Path = ")}


def extract(seven_zip, raw, path):
    return subprocess.check_output([seven_zip, "x", "-thfs", "-so", raw, path])


def positions(data):
    with DSStore.open(BytesIO(data), "r") as store:
        return [(entry.filename, entry.value) for entry in store if entry.code == b"Iloc"]


def verify(binary, seven_zip, root, image, name, xy, nested_xy):
    if sys.platform == "darwin":
        subprocess.run(["/usr/bin/hdiutil", "verify", "-nocache", image], cwd=root, check=True)
    raw = root / Path(image).with_suffix(".raw")
    subprocess.run([binary, "unwrap-raw", image, "--output", raw], cwd=root, check=True)
    catalog = entries(seven_zip, raw)
    assert f"{VOLUME}/{name}" in catalog, catalog
    assert f"{VOLUME}/Guide/{NESTED_NAME}" in catalog, catalog
    # Match payloads using their unique ASCII prefixes because host argument
    # normalization can change a complete Unicode 7-Zip selector.
    assert extract(seven_zip, raw, f"{VOLUME}/{name[0]}*") == PAYLOAD
    assert extract(seven_zip, raw, f"{VOLUME}/Guide/{NESTED_NAME[0]}*") == PAYLOAD
    root_positions = positions(extract(seven_zip, raw, f"{VOLUME}/.DS_Store"))
    nested_positions = positions(extract(seven_zip, raw, f"{VOLUME}/Guide/.DS_Store"))
    assert root_positions == [(name, xy)], root_positions
    assert nested_positions == [(NESTED_NAME, nested_xy)], nested_positions


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dmgez", type=Path)
    parser.add_argument("--seven-zip", default="7z")
    args = parser.parse_args()
    binary = args.dmgez.resolve(strict=True)
    assert version("ds_store") == "1.3.4"
    assert version("mac_alias") == "2.2.3"
    with TemporaryDirectory(prefix="dmgez-finder-unicode-") as temporary:
        root = Path(temporary)
        (root / "dmg.toml").write_text(PROJECT, encoding="utf-8")
        (root / "edits.toml").write_text(EDITS, encoding="utf-8")
        (root / "payload.bin").write_bytes(PAYLOAD)
        shutil.copyfile(BACKGROUND, root / "background.png")

        subprocess.run([binary, "build", "dmg.toml"], cwd=root, check=True)
        verify(binary, args.seven_zip, root, "original.dmg", ORIGINAL_NAME, (17, 23), (31, 47))
        source = (root / "original.dmg").read_bytes()
        subprocess.run(
            [binary, "edit-image", "original.dmg", "--changes", "edits.toml", "--output", "edited.dmg"],
            cwd=root,
            check=True,
        )
        verify(binary, args.seven_zip, root, "edited.dmg", RENAMED_NAME, (29, 53), (59, 61))
        assert (root / "original.dmg").read_bytes() == source
    print("Verified exact Unicode Finder position names in original and edited images")


if __name__ == "__main__":
    main()
