#!/usr/bin/env python3
"""Check Finder layout comparison on disposable images with an independent decoder."""

import argparse
from importlib.metadata import version
import json
import os
from pathlib import Path
import plistlib
import sys
from tempfile import TemporaryDirectory

from ds_store import DSStore

from check_apfs_cli_multivolume import SW_VERS, UNAME, digest, run
from check_extract_quarantine import HDIUTIL
from check_semantic_diff import mounted


IMAGE_SIZE = "128m"
FOLDER = "Payload"
ITEM = "Example"
STORE = ".DS_Store"
CURRENT = "."
WINDOW = "bwsp"
VIEW = "icvl"
POSITION = "Iloc"
LIST_KEYED = "lsvp"
LIST_ORDERED = "lsvC"
LIST_VIEW = b"Nlsv"
VERSION = "vSrn"
DS_STORE_VERSION = "1.3.4"
MAC_ALIAS_VERSION = "2.2.3"
ORIGINAL_POSITION = (150, 175)
REVISED_POSITION = (200, 175)
ORIGINAL_WIDTH = 245
REVISED_WIDTH = 285
UNALLOCATED_BYTES = b"synthetic unused tail\x00\xff"


def make_store(path, position, width):
    window = {
        "WindowBounds": "{{75, -50}, {620, 270}}",
        "ShowStatusBar": True,
        "ShowToolbar": False,
        "PreviewPaneVisibility": False,
    }
    settings = {
        "calculateAllSizes": True,
        "iconSize": 32.0,
        "scrollPositionX": 2.5,
        "scrollPositionY": 3.5,
        "showIconPreview": True,
        "sortColumn": "size",
        "textSize": 13.0,
        "useRelativeDates": False,
        "viewOptionsVersion": 1,
    }
    columns = {
        "name": {"width": width, "ascending": True, "visible": True},
        "size": {"width": 97, "ascending": False, "visible": True},
    }
    keyed = {name: {**value, "index": index} for index, (name, value) in enumerate(columns.items())}
    ordered = [{**value, "identifier": name} for name, value in columns.items()]

    with DSStore.open(str(path), "w+") as store:
        store[CURRENT][VERSION] = ("long", 1)
        store[CURRENT][VIEW] = ("type", LIST_VIEW)
        store[CURRENT][WINDOW] = window
        store[ITEM][POSITION] = position
        store[CURRENT][LIST_KEYED] = {**settings, "columns": keyed}
        store[CURRENT][LIST_ORDERED] = (
            "blob", plistlib.dumps({**settings, "columns": ordered}, fmt=plistlib.FMT_BINARY),
        )


def read_facts(path):
    with DSStore.open(str(path), "r") as store:
        assert store[CURRENT][VIEW] == (b"type", LIST_VIEW)
        assert store[CURRENT][VERSION] == (b"long", 1)
        tag, value = store[CURRENT][LIST_ORDERED]
        assert tag == b"blob"
        return {
            "window": store[CURRENT][WINDOW],
            "position": list(store[ITEM][POSITION]),
            "list_keyed": store[CURRENT][LIST_KEYED],
            "list_ordered": plistlib.loads(bytes(value)),
        }


def assign_store(root, data):
    directory = root / FOLDER
    if not directory.exists():
        directory.mkdir()
        (directory / ITEM).write_bytes(b"Unchanged item contents\n")
    (directory / STORE).write_bytes(data)


def compare(cli, left, right, expected, finder):
    options = ("--finder-layout",) if finder else ()
    result = run(
        cli, "diff", left, right, "--mode", "semantic", "--filesystem", "apfs",
        "--json", *options, check=False,
    )
    assert result.returncode == expected, result
    report = json.loads(result.stdout)
    assert report["outcome"] == ("equal" if expected == 0 else "different"), report
    return report


def qualify(cli, root):
    original_store = root / "original-store"
    revised_store = root / "revised-store"
    make_store(original_store, ORIGINAL_POSITION, ORIGINAL_WIDTH)
    make_store(revised_store, REVISED_POSITION, REVISED_WIDTH)
    original_bytes = original_store.read_bytes()
    revised_bytes = revised_store.read_bytes()
    variants = (
        ("original", original_bytes),
        ("padded", original_bytes + UNALLOCATED_BYTES),
        ("revised", revised_bytes),
    )
    source = root / "source.dmg"
    run(
        HDIUTIL, "create", "-size", IMAGE_SIZE, "-fs", "APFS", "-layout", "NONE",
        "-type", "UDIF", "-volname", "Pomologie Finder Diff", "-nospotlight", source,
    )
    images = {}
    native = {}
    verified = {}
    for name, data in variants:
        image = root / f"{name}.dmg"
        mounted(source, root / f"{name}-write", "-readwrite", lambda path: assign_store(path, data))
        run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", image, "-quiet")
        images[name] = image
        verified[name] = run(HDIUTIL, "verify", image).stdout.decode().splitlines()
        mounted(
            image, root / f"{name}-read", "-readonly",
            lambda path: native.update({name: read_facts(path / FOLDER / STORE)}),
        )
    assert native["original"] == native["padded"]
    assert native["original"]["position"] == list(ORIGINAL_POSITION)
    assert native["revised"]["position"] == list(REVISED_POSITION)
    for name, width in (("original", ORIGINAL_WIDTH), ("revised", REVISED_WIDTH)):
        assert native[name]["list_keyed"]["columns"]["name"]["width"] == width
        assert native[name]["list_ordered"]["columns"][0]["width"] == width

    original, padded, revised = (images[name] for name, _ in variants)
    digests = {name: digest(path) for name, path in images.items()}
    assert len(set(digests.values())) == len(images)
    baseline = compare(cli, original, padded, 1, False)
    assert baseline["changes"] == [{
        "path": [FOLDER, STORE], "kind": "changed", "fields": ["data_fork"],
    }], baseline
    equal = compare(cli, original, padded, 0, True)
    assert equal["changes"] == [], equal
    different = compare(cli, original, revised, 1, True)
    [change] = different["changes"]
    assert change["path"] == [FOLDER, STORE] and change["fields"] == ["finder_layout"], change
    assert {bytes(record["field"]).decode("ascii") for record in change["finder_layout"]} == {
        POSITION, LIST_KEYED, LIST_ORDERED,
    }, change
    assert {name: digest(path) for name, path in images.items()} == digests
    return {
        "macos_version": run(SW_VERS, "-productVersion").stdout.decode().strip(),
        "macos_build": run(SW_VERS, "-buildVersion").stdout.decode().strip(),
        "architecture": run(UNAME, "-m").stdout.decode().strip(),
        "independent_packages": {
            "ds_store": version("ds_store"), "mac_alias": version("mac_alias"),
        },
        "image_digests": digests,
        "input_images_unchanged": True,
        "native_udzo_verification": verified,
        "independent_facts": native,
        "default_semantic_delta": baseline,
        "finder_equal": equal,
        "finder_different": different,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("the Finder layout oracle requires macOS")
    assert version("ds_store") == DS_STORE_VERSION
    assert version("mac_alias") == MAC_ALIAS_VERSION
    cli = args.cli.resolve(strict=True)
    os.umask(0o077)
    with TemporaryDirectory(prefix="dmgez-native-finder-diff.", dir="/private/tmp") as scratch:
        print(json.dumps(qualify(cli, Path(scratch)), indent=2))


if __name__ == "__main__":
    main()
