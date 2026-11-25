#!/usr/bin/env python3
"""Qualify a TOML list layout through the project runner and a native mount."""

import argparse
from importlib.metadata import version
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory

from ds_store import DSStore

BASE = Path(__file__).resolve().parent
FIXTURE = BASE / "tests" / "fixtures" / "finder-list.toml"
IMAGE = "list-layout.dmg"
HDIUTIL = "/usr/bin/hdiutil"
STORE = ".DS_Store"
ROOT_KEY = "."
LIST_COMMON = {
    "calculateAllSizes": True,
    "iconSize": 32.0,
    "scrollPositionX": 2.5,
    "scrollPositionY": 3.5,
    "showIconPreview": True,
    "sortColumn": "size",
    "useRelativeDates": False,
    "viewOptionsVersion": 1,
}
DEFAULT_COLUMNS = (
    ("name", 300, True),
    ("dateModified", 181, False),
    ("dateCreated", 181, False),
    ("dateAdded", 181, False),
    ("dateLastOpened", 200, False),
    ("size", 97, False),
    ("kind", 115, True),
    ("label", 100, True),
    ("version", 75, True),
    ("comments", 300, True),
)


def verify_store(path, visible, size, widths, directions, icon_anchor=None):
    with DSStore.open(str(path), "r") as store:
        assert store[ROOT_KEY]["icvl"] == (b"type", b"Nlsv")
        assert store[ROOT_KEY]["vSrn"] == (b"long", 1)
        keyed = store[ROOT_KEY]["lsvp"]
        tag, data = store[ROOT_KEY]["lsvC"]
        assert tag == b"blob"
        ordered = plistlib.loads(bytes(data))
        if icon_anchor:
            assert store["Icon Anchor"]["Iloc"] == icon_anchor

    expected = {**LIST_COMMON, "textSize": size}
    for options in (keyed, ordered):
        settings = {name: value for name, value in options.items() if name != "columns"}
        assert settings == expected
        assert all(type(settings[name]) is type(value) for name, value in expected.items())

    defaults = {name: (width, ascending) for name, width, ascending in DEFAULT_COLUMNS}
    order = [*visible, *(name for name in defaults if name not in visible)]
    common = {
        name: {
            "width": widths.get(name, defaults[name][0]),
            "ascending": directions.get(name, defaults[name][1]),
            "visible": name in visible,
        }
        for name in order
    }
    assert keyed["columns"] == {
        name: {**common[name], "index": index} for index, name in enumerate(order)
    }
    assert ordered["columns"] == [
        {**common[name], "identifier": name} for name in order
    ]
    for column in ordered["columns"]:
        assert type(column["width"]) is int
        assert type(column["ascending"]) is bool
        assert type(column["visible"]) is bool


def qualify_mount(image, mount):
    subprocess.run([HDIUTIL, "verify", image], check=True)
    mount.mkdir()
    attached = subprocess.check_output([
        HDIUTIL, "attach", "-readonly", "-nobrowse", "-noautoopen", "-mount", "required",
        "-mountpoint", mount, "-plist", image,
    ])
    entities = plistlib.loads(attached)["system-entities"]
    whole_devices = [
        entry["dev-entry"] for entry in entities
        if re.fullmatch(r"/dev/disk[0-9]+", entry.get("dev-entry", ""))
    ]
    try:
        assert len(whole_devices) == 1
        assert mount.resolve() in {
            Path(entry["mount-point"]).resolve()
            for entry in entities if "mount-point" in entry
        }
        verify_store(
            mount / STORE, ["name", "dateAdded", "size"], 13.0,
            {"name": 245, "size": 140, "dateLastOpened": 211},
            {"size": True, "dateLastOpened": True}, icon_anchor=(55, 70),
        )
        verify_store(
            mount / "Documentation" / STORE, ["name", "size"], 14.0,
            {"name": 245, "size": 200, "dateLastOpened": 212},
            {"size": False, "dateLastOpened": True},
        )
    finally:
        for device in whole_devices:
            subprocess.run([HDIUTIL, "detach", device], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dmgez", type=Path, required=True)
    parser.add_argument("--hfsplus", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("the native Finder list check requires macOS")
    assert version("ds_store") == "1.3.4"
    assert version("mac_alias") == "2.2.3"
    with TemporaryDirectory(prefix="dmgez-finder-list-") as temporary:
        root = Path(temporary)
        project = root / FIXTURE.name
        shutil.copy2(FIXTURE, project)
        runner = [
            sys.executable, BASE / "run.py", "--dmgez", args.dmgez, "--hfsplus",
            args.hfsplus, "--project", project,
        ]
        dry_run = subprocess.check_output([*runner, "--dry-run"], text=True)
        assert {"finder=ListView", "finder_assets=2", "logical_bytes=1048576"} <= set(dry_run.splitlines())
        assert list(root.iterdir()) == [project]
        subprocess.run(runner, check=True)
        qualify_mount(root / IMAGE, root / "mount")
    print("Verified both mounted Finder list layouts")


if __name__ == "__main__":
    main()
