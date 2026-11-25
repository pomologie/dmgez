#!/usr/bin/env python3
"""Check semantic diff against independently edited native APFS images."""

import argparse
import json
import os
from pathlib import Path
import stat
import sys
from tempfile import TemporaryDirectory

from check_apfs_cli_multivolume import FSCK, SW_VERS, UNAME, digest, run
from check_apfs_extraction import RESOURCE_ATTRIBUTE
from check_extract_quarantine import HDIUTIL, XATTR, attach


IMAGE_SIZE = "128m"
PAYLOAD = "Payload"
DATA_BEFORE = b"first\n"
DATA_AFTER = b"other\n"
RESOURCE_BEFORE = b"first APFS resource\0\n"
RESOURCE_AFTER = b"other APFS resource\0\n"
CONSTANT_DATA = b"unchanged ordinary data\n"
MODE_BEFORE = 0o600
MODE_AFTER = 0o640
OLD_NAME = "Old.txt"
NEW_NAME = "New.txt"
CONTENTS_OPTION = "--contents"
HASH_MODE = "blake3"
CONFIRM_MODE = "blake3-exact"
CONFIRMED_POLICY = "blake3_exact"
INFER_RENAMES = "--infer-renames"
RENAME_POLICY = "unique_blake3_forks"


def set_resource(path, contents):
    run(XATTR, "-wx", RESOURCE_ATTRIBUTE, contents.hex(), path)


def create_initial(root):
    directory = root / PAYLOAD
    directory.mkdir()
    (directory / "Data.txt").write_bytes(DATA_BEFORE)
    for name in ("Resource.txt", "Mode.txt", "Shared.txt", OLD_NAME):
        (directory / name).write_bytes(CONSTANT_DATA)
    set_resource(directory / "Resource.txt", RESOURCE_BEFORE)
    (directory / "Mode.txt").chmod(MODE_BEFORE)
    (directory / "Link").symlink_to("Mode.txt")
    os.link(directory / "Shared.txt", directory / "Alias.txt")


def edit(root):
    directory = root / PAYLOAD
    (directory / "Data.txt").write_bytes(DATA_AFTER)
    set_resource(directory / "Resource.txt", RESOURCE_AFTER)
    (directory / "Mode.txt").chmod(MODE_AFTER)
    (directory / "Link").unlink()
    (directory / "Link").symlink_to("Data.txt")
    (directory / "Alias.txt").unlink()
    (directory / "Alias.txt").write_bytes(CONSTANT_DATA)
    (directory / OLD_NAME).rename(directory / NEW_NAME)


def check_native(root, changed):
    directory = root / PAYLOAD
    assert (directory / "Data.txt").read_bytes() == (DATA_AFTER if changed else DATA_BEFORE)
    resource = run(XATTR, "-px", RESOURCE_ATTRIBUTE, directory / "Resource.txt").stdout.decode()
    assert bytes.fromhex(resource) == (RESOURCE_AFTER if changed else RESOURCE_BEFORE)
    assert (directory / "Resource.txt").read_bytes() == CONSTANT_DATA
    mode = stat.S_IMODE((directory / "Mode.txt").stat().st_mode)
    assert mode == (MODE_AFTER if changed else MODE_BEFORE)
    assert (directory / "Mode.txt").read_bytes() == CONSTANT_DATA
    assert os.readlink(directory / "Link") == ("Data.txt" if changed else "Mode.txt")
    shared = directory / "Shared.txt"
    alias = directory / "Alias.txt"
    assert shared.read_bytes() == alias.read_bytes() == CONSTANT_DATA
    assert (shared.stat().st_ino == alias.stat().st_ino) is (not changed)
    before, after = directory / OLD_NAME, directory / NEW_NAME
    assert (after if changed else before).read_bytes() == CONSTANT_DATA
    assert not (before if changed else after).exists()


def mounted(image, mount, mode, action):
    device = attach(image, mount, mode)
    try:
        action(mount)
    finally:
        run(HDIUTIL, "detach", device)


def diff(cli, left, right, expected, *options):
    result = run(
        cli, "diff", left, right, "--mode", "semantic", "--filesystem", "apfs",
        "--json", *options, check=False,
    )
    assert result.returncode == expected, result
    return json.loads(result.stdout)


def check_changes(report):
    payload_changes = {
        change["path"][1]: (change["kind"], change["fields"])
        for change in report["changes"]
        if change["path"][:1] == [PAYLOAD] and len(change["path"]) == 2
    }
    assert payload_changes == {
        "Alias.txt": ("changed", ["hard_link_topology"]),
        "Data.txt": ("changed", ["data_fork"]),
        "Link": ("changed", ["symlink_target"]),
        "Mode.txt": ("changed", ["mode"]),
        NEW_NAME: ("added", []),
        OLD_NAME: ("removed", []),
        "Resource.txt": ("changed", ["resource_fork"]),
        "Shared.txt": ("changed", ["hard_link_topology"]),
    }, report


def check_hash_modes(cli, uncompressed, initial, revised, baseline):
    physical = run(cli, "diff", uncompressed, initial, "--mode", "physical", check=False)
    assert physical.returncode == 1, physical
    hashed = diff(cli, uncompressed, initial, 0, CONTENTS_OPTION, HASH_MODE)
    confirmed = diff(cli, uncompressed, initial, 0, CONTENTS_OPTION, CONFIRM_MODE)
    for comparison, policy in ((hashed, HASH_MODE), (confirmed, CONFIRMED_POLICY)):
        assert comparison["outcome"] == "equal" and comparison["changes"] == [], comparison
        assert comparison["policy"]["contents"] == policy, comparison
    assert int(confirmed["compared_bytes"]) > int(hashed["compared_bytes"])

    flags = (CONTENTS_OPTION, HASH_MODE, INFER_RENAMES)
    edited = diff(cli, uncompressed, revised, 1, *flags)
    assert edited["outcome"] == "different" and edited["changes"] == baseline["changes"], edited
    assert edited["policy"]["rename_inference"] == RENAME_POLICY, edited
    assert edited["rename_candidates"] == [{
        "from": [PAYLOAD, OLD_NAME], "to": [PAYLOAD, NEW_NAME],
    }], edited
    assert diff(cli, uncompressed, revised, 1, *flags) == edited
    return {
        "different_storage_exit_code": physical.returncode,
        "blake3_equal": hashed,
        "confirmed_equal": confirmed,
        "inferred_renames": edited,
    }


def qualify(cli, root, hash_comparison=False):
    source = root / "source.dmg"
    raw = root / "initial.raw.cdr"
    uncompressed = root / "initial-udro.dmg"
    initial = root / "initial-udzo.dmg"
    revised = root / "revised-udzo.dmg"
    run(
        HDIUTIL, "create", "-size", IMAGE_SIZE, "-fs", "APFS", "-layout", "NONE",
        "-type", "UDIF", "-volname", "Pomologie Semantic Diff", "-nospotlight", source,
    )
    mounted(source, root / "initial-write", "-readwrite", create_initial)
    run(HDIUTIL, "convert", source, "-format", "UDTO", "-o", root / "initial.raw", "-quiet")
    run(HDIUTIL, "convert", source, "-format", "UDRO", "-o", uncompressed, "-quiet")
    run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", initial, "-quiet")
    mounted(source, root / "revised-write", "-readwrite", edit)
    run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", revised, "-quiet")
    mounted(initial, root / "initial-read", "-readonly", lambda path: check_native(path, False))
    mounted(revised, root / "revised-read", "-readonly", lambda path: check_native(path, True))
    native_check = run(FSCK, "-n", raw)
    uncompressed_check = run(HDIUTIL, "verify", uncompressed)
    initial_check = run(HDIUTIL, "verify", initial)
    revised_check = run(HDIUTIL, "verify", revised)
    assert f"The container {raw} appears to be OK" in native_check.stdout.decode()
    paths = (raw, uncompressed, initial, revised)
    before = tuple(digest(path) for path in paths)
    assert len(set(before)) == len(paths)
    raw_equal = diff(cli, raw, initial, 0, "--left-raw")
    wrapped_equal = diff(cli, uncompressed, initial, 0)
    changed = diff(cli, raw, revised, 1, "--left-raw")
    for result in (raw_equal, wrapped_equal):
        assert result["outcome"] == "equal" and result["changes"] == [], result
    assert changed["outcome"] == "different", changed
    check_changes(changed)
    assert diff(cli, raw, revised, 1, "--left-raw") == changed
    hashes = check_hash_modes(cli, uncompressed, initial, revised, changed) if hash_comparison else {}
    assert tuple(digest(path) for path in paths) == before
    if hash_comparison:
        hashes["input_images_unchanged"] = True
    return {
        "macos_version": run(SW_VERS, "-productVersion").stdout.decode().strip(),
        "macos_build": run(SW_VERS, "-buildVersion").stdout.decode().strip(),
        "architecture": run(UNAME, "-m").stdout.decode().strip(),
        "image_digests": dict(zip((path.name for path in paths), before, strict=True)),
        "fsck_stdout": native_check.stdout.decode().splitlines(),
        "fsck_stderr": native_check.stderr.decode().splitlines(),
        "initial_udro_verify": uncompressed_check.stdout.decode().splitlines(),
        "initial_udzo_verify": initial_check.stdout.decode().splitlines(),
        "revised_udzo_verify": revised_check.stdout.decode().splitlines(),
        "raw_equal": raw_equal,
        "wrapped_equal": wrapped_equal,
        "different": changed,
        **hashes,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--hash", action="store_true", help="exercise BLAKE3 and rename inference")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("the native comparison requires macOS")
    cli = args.cli.resolve(strict=True)
    os.umask(0o077)
    with TemporaryDirectory(prefix="dmgez-native-semantic-diff.", dir="/private/tmp") as scratch:
        print(json.dumps(qualify(cli, Path(scratch), args.hash), indent=2))


if __name__ == "__main__":
    main()
