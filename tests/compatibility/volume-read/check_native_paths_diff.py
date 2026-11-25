#!/usr/bin/env python3
"""Check native path matching against read-only HFS+ and HFSX images."""

import argparse
import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory

from check_apfs_cli_multivolume import SW_VERS, UNAME, digest, run
from check_extract_quarantine import HDIUTIL
from check_semantic_diff import mounted


IMAGE_SIZE = "128m"
FOLDER_INPUT = "\u00c9tage"
FOLDER_BEFORE = "E\u0301tage"
FOLDER_AFTER = "e\u0301TAGE"
FOLDER_ALTERNATE = "\u00c9TAGE"
FILE_BEFORE = "Report.txt"
FILE_AFTER = "rePORT.txt"
FILE_ALTERNATE = "REPORT.TXT"
STABLE_FILE = "Stable.bin"
TEMP_NAME = "RenameIntermediary"
ORIGINAL_DATA = b"Before\n"
REVISED_DATA = b"Alter!\n"
STABLE_DATA = b"Same data\0\n"
NATIVE_PATHS = "--paths"
NATIVE_MODE = "native"


def create_initial(root):
    folder = root / FOLDER_INPUT
    folder.mkdir()
    (folder / FILE_BEFORE).write_bytes(ORIGINAL_DATA)
    (folder / STABLE_FILE).write_bytes(STABLE_DATA)


def rename_case(source, destination):
    intermediate = source.with_name(TEMP_NAME)
    assert not intermediate.exists()
    source.rename(intermediate)
    intermediate.rename(destination)


def edit(root):
    folder = root / FOLDER_AFTER
    rename_case(root / FOLDER_BEFORE, folder)
    rename_case(folder / FILE_BEFORE, folder / FILE_AFTER)
    (folder / FILE_AFTER).write_bytes(REVISED_DATA)


def native_facts(root, revised, folded):
    folder_name = FOLDER_AFTER if revised else FOLDER_BEFORE
    file_name = FILE_AFTER if revised else FILE_BEFORE
    folder = root / folder_name
    file = folder / file_name
    assert folder_name in os.listdir(root), os.listdir(root)
    assert sorted(os.listdir(folder)) == sorted((file_name, STABLE_FILE))
    assert file.read_bytes() == (REVISED_DATA if revised else ORIGINAL_DATA)
    assert (folder / STABLE_FILE).read_bytes() == STABLE_DATA

    alternate = root / FOLDER_ALTERNATE / FILE_ALTERNATE
    if folded:
        assert alternate.stat().st_ino == file.stat().st_ino
    else:
        assert not alternate.exists()
    return {
        "directory": folder_name,
        "file": file_name,
        "file_payload": file.read_bytes().hex(),
        "stable_payload": (folder / STABLE_FILE).read_bytes().hex(),
        "alternate_spelling_resolves": folded,
    }


def compare(cli, left, right, *options, expected=1):
    result = run(
        cli, "diff", left, right, "--mode", "semantic", "--filesystem", "hfsplus",
        "--json", *options, check=False,
    )
    assert result.returncode == expected, result
    return json.loads(result.stdout)


def expect_stored_path_changes(report):
    def paths(folder, name):
        return [(folder,), (folder, name), (folder, STABLE_FILE)]

    expected = {
        **{path: "removed" for path in paths(FOLDER_BEFORE, FILE_BEFORE)},
        **{path: "added" for path in paths(FOLDER_AFTER, FILE_AFTER)},
    }
    actual = {tuple(change["path"]): change["kind"] for change in report["changes"]}
    assert report["outcome"] == "different" and actual == expected, report


def expect_native_changes(report, folded):
    expected_profile = "hfs_unicode_folded" if folded else "hfsx_binary"
    assert report["policy"]["paths"] == NATIVE_MODE, report
    assert report["policy"]["native_name_profile"] == expected_profile, report
    if not folded:
        expect_stored_path_changes(report)
        return

    assert report["changes"] == [{
        "path": [FOLDER_BEFORE, FILE_BEFORE], "kind": "changed", "fields": ["data_fork"],
        "right_path": [FOLDER_AFTER, FILE_AFTER],
    }], report
    assert report["equivalent_path_spellings"] == [
        {"left": [FOLDER_BEFORE], "right": [FOLDER_AFTER]},
        {"left": [FOLDER_BEFORE, STABLE_FILE], "right": [FOLDER_AFTER, STABLE_FILE]},
    ], report


def qualify(cli, root, fs, folded):
    root.mkdir()
    source = root / "source.dmg"
    initial = root / "initial.dmg"
    revised = root / "revised.dmg"
    run(
        HDIUTIL, "create", "-size", IMAGE_SIZE, "-fs", fs, "-layout", "NONE",
        "-type", "UDIF", "-volname", "Pomologie Native Paths", "-nospotlight", source,
    )
    mounted(source, root / "initial-write", "-readwrite", create_initial)
    run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", initial, "-quiet")
    mounted(source, root / "revised-write", "-readwrite", edit)
    run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", revised, "-quiet")
    images = (initial, revised)
    before = {path.name: digest(path) for path in images}
    assert len(set(before.values())) == len(images)
    facts = {}
    verification = {}
    for image, changed in zip(images, (False, True), strict=True):
        verification[image.name] = run(HDIUTIL, "verify", image).stdout.decode().splitlines()
        mounted(
            image, root / f"{image.stem}-read", "-readonly",
            lambda path: facts.update({image.name: native_facts(path, changed, folded)}),
        )

    report = {"image_digests": before, "native_udzo_verification": verification, "facts": facts}
    if cli is not None:
        exact = compare(cli, initial, revised)
        expect_stored_path_changes(exact)
        native = compare(cli, initial, revised, NATIVE_PATHS, NATIVE_MODE)
        expect_native_changes(native, folded)
        assert compare(cli, initial, revised, NATIVE_PATHS, NATIVE_MODE) == native
        report.update({"exact": exact, "native": native})
    assert {path.name: digest(path) for path in images} == before
    report["input_images_unchanged"] = True
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--cli", type=Path)
    mode.add_argument("--native-only", action="store_true")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("the native path oracle requires macOS")
    cli = args.cli.resolve(strict=True) if args.cli is not None else None
    os.umask(0o077)
    with TemporaryDirectory(prefix="dmgez-native-paths.", dir="/private/tmp") as scratch:
        root = Path(scratch)
        print(json.dumps({
            "macos_version": run(SW_VERS, "-productVersion").stdout.decode().strip(),
            "macos_build": run(SW_VERS, "-buildVersion").stdout.decode().strip(),
            "architecture": run(UNAME, "-m").stdout.decode().strip(),
            "filesystems": {
                "hfsplus": qualify(cli, root / "hfsplus", "HFS+", True),
                "hfsx": qualify(cli, root / "hfsx", "Case-sensitive HFS+", False),
            },
        }, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
