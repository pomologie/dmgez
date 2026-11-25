#!/usr/bin/env python3
"""Compare per-object FinderInfo with native macOS results on disposable images."""

import argparse
import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory

from check_apfs_cli_multivolume import SW_VERS, UNAME, digest, run
from check_apfs_extraction import RESOURCE_ATTRIBUTE
from check_extract_quarantine import HDIUTIL, XATTR
from check_semantic_diff import mounted


IMAGE_SIZE = "128m"
PAYLOAD = "Payload"
ITEM = "Item.bin"
ALIAS = "Alias.bin"
PLAIN = "Plain.bin"
ADDED = "Added.bin"
REMOVED = "Removed.bin"
LINK = "Link"
FINDER_INFO = "com.apple.FinderInfo"
FINDER_INFO_BYTES = 32
FINDER_FLAGS = slice(8, 10)
LABEL_BEFORE = 0x0002
LABEL_AFTER = 0x0004
TYPE_CREATOR_BEFORE = b"TEXTttxt"
TYPE_CREATOR_AFTER = b"BINAdiff"
FILE_DATA = b"Unchanged file bytes\n"
RESOURCE_DATA = b"Unchanged resource fork\0"
FINDER_OPTION = "--finder-info"
FINDER_FIELD = "finder_info"
XATTR_OPTION = "--xattrs"


def finder_info(flags, type_creator=b""):
    data = bytearray(FINDER_INFO_BYTES)
    data[:len(type_creator)] = type_creator
    data[FINDER_FLAGS] = flags.to_bytes(FINDER_FLAGS.stop - FINDER_FLAGS.start, "big")
    return bytes(data)


def write_info(path, value, symlink=False):
    run(XATTR, "-swx" if symlink else "-wx", FINDER_INFO, value.hex(), path)


def read_info(path, symlink=False):
    args = ("-s",) if symlink else ()
    names = run(XATTR, *args, path).stdout.decode().splitlines()
    if FINDER_INFO not in names:
        return None
    result = run(XATTR, "-spx" if symlink else "-px", FINDER_INFO, path)
    value = bytes.fromhex(result.stdout.decode())
    assert len(value) == FINDER_INFO_BYTES, (path.name, len(value))
    return value


def create_initial(root):
    directory = root / PAYLOAD
    directory.mkdir()
    for name in (ITEM, PLAIN, ADDED, REMOVED):
        (directory / name).write_bytes(FILE_DATA)
    os.link(directory / ITEM, directory / ALIAS)
    (directory / LINK).symlink_to(PLAIN)
    write_info(directory, finder_info(LABEL_BEFORE))
    write_info(directory / ITEM, finder_info(LABEL_BEFORE, TYPE_CREATOR_BEFORE))
    write_info(directory / REMOVED, finder_info(LABEL_BEFORE, TYPE_CREATOR_BEFORE))
    write_info(directory / LINK, finder_info(LABEL_BEFORE), symlink=True)
    run(XATTR, "-wx", RESOURCE_ATTRIBUTE, RESOURCE_DATA.hex(), directory / ITEM)


def edit(root):
    directory = root / PAYLOAD
    write_info(directory, finder_info(LABEL_AFTER))
    write_info(directory / ALIAS, finder_info(LABEL_AFTER, TYPE_CREATOR_AFTER))
    write_info(directory / ADDED, finder_info(LABEL_AFTER, TYPE_CREATOR_AFTER))
    write_info(directory / PLAIN, bytes(FINDER_INFO_BYTES))
    run(XATTR, "-d", FINDER_INFO, directory / REMOVED)
    write_info(directory / LINK, finder_info(LABEL_AFTER), symlink=True)


def check_native(root, revised):
    directory = root / PAYLOAD
    file_info = finder_info(
        LABEL_AFTER if revised else LABEL_BEFORE,
        TYPE_CREATOR_AFTER if revised else TYPE_CREATOR_BEFORE,
    )
    flags_info = finder_info(LABEL_AFTER if revised else LABEL_BEFORE)
    expected = {
        directory: flags_info,
        directory / ITEM: file_info,
        directory / ALIAS: file_info,
        directory / PLAIN: None,
        directory / ADDED: file_info if revised else None,
        directory / REMOVED: None if revised else file_info,
        directory / LINK: flags_info,
    }
    observed = {}
    for path, value in expected.items():
        actual = read_info(path, symlink=path.name == LINK)
        assert actual == value, (path.name, actual, value)
        observed[path.relative_to(root).as_posix()] = actual.hex() if actual is not None else None
    for name in (ITEM, ALIAS, PLAIN, ADDED, REMOVED):
        assert (directory / name).read_bytes() == FILE_DATA, name
    item, alias = (directory / ITEM).stat(), (directory / ALIAS).stat()
    assert item.st_ino == alias.st_ino and item.st_nlink == alias.st_nlink == 2
    for name in (ITEM, ALIAS):
        fork = run(XATTR, "-px", RESOURCE_ATTRIBUTE, directory / name)
        assert bytes.fromhex(fork.stdout.decode()) == RESOURCE_DATA, name
    assert os.readlink(directory / LINK) == PLAIN
    return observed


def comparison(cli, left, right, filesystem, *options):
    finder = FINDER_OPTION in options
    result = run(
        cli, "diff", left, right, "--mode", "semantic", "--filesystem", filesystem,
        "--json", *options, check=False,
    )
    assert result.returncode == int(finder), result
    report = json.loads(result.stdout)
    assert report["outcome"] == ("different" if finder else "equal"), report
    if finder:
        assert report["policy"][FINDER_FIELD] == {
            "comparison": "exact_effective_bytes",
            "record_bytes": FINDER_INFO_BYTES,
            "same_kind_only": True,
        }, report
        assert FINDER_FIELD in report["policy"]["compared"], report
    else:
        assert FINDER_FIELD not in report["policy"], report
    assert ("xattrs" in report["policy"]) == (XATTR_OPTION in options), report
    paths = [(PAYLOAD,)] + [(PAYLOAD, name) for name in (ITEM, ALIAS, ADDED, REMOVED, LINK)]
    expected = [
        {"path": list(path), "kind": "changed", "fields": [FINDER_FIELD]}
        for path in sorted(paths)
    ]
    assert report["changes"] == (expected if finder else []), report
    return report


def qualify(cli, root, filesystem, native_filesystem):
    root.mkdir()
    source = root / "source.dmg"
    left = root / "initial.dmg"
    right = root / "revised.dmg"
    run(
        HDIUTIL, "create", "-size", IMAGE_SIZE, "-fs", native_filesystem, "-layout", "NONE",
        "-type", "UDIF", "-volname", "Pomologie FinderInfo", "-nospotlight", source,
    )
    mounted(source, root / "initial-write", "-readwrite", create_initial)
    run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", left, "-quiet")
    mounted(source, root / "revised-write", "-readwrite", edit)
    run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", right, "-quiet")
    variants = (left, right)
    before = {path.name: digest(path) for path in variants}
    assert len(set(before.values())) == len(variants)
    observed = {}
    checks = {}
    for path, revised in zip(variants, (False, True), strict=True):
        checks[path.name] = run(HDIUTIL, "verify", path).stdout.decode().splitlines()
        mounted(
            path, root / f"{path.stem}-read", "-readonly",
            lambda mount: observed.update({path.name: check_native(mount, revised)}),
        )
    evidence = {
        "image_digests": before,
        "native_udzo_verification": checks,
        "native_finder_info": observed,
    }
    if cli is not None:
        evidence["comparisons"] = {
            "baseline": comparison(cli, left, right, filesystem),
            "xattrs": comparison(cli, left, right, filesystem, XATTR_OPTION),
            "finder_info": comparison(cli, left, right, filesystem, FINDER_OPTION),
            "finder_info_and_xattrs": comparison(
                cli, left, right, filesystem, FINDER_OPTION, XATTR_OPTION,
            ),
        }
    assert {path.name: digest(path) for path in variants} == before
    evidence["input_images_unchanged"] = True
    return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--cli", type=Path)
    mode.add_argument("--native-only", action="store_true")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("the native FinderInfo comparison requires macOS")
    cli = args.cli.resolve(strict=True) if args.cli is not None else None
    os.umask(0o077)
    with TemporaryDirectory(prefix="dmgez-native-finder-info.", dir="/private/tmp") as scratch:
        root = Path(scratch)
        print(json.dumps({
            "macos_version": run(SW_VERS, "-productVersion").stdout.decode().strip(),
            "macos_build": run(SW_VERS, "-buildVersion").stdout.decode().strip(),
            "architecture": run(UNAME, "-m").stdout.decode().strip(),
            "filesystems": {
                profile: qualify(cli, root / profile, profile, native)
                for profile, native in (("apfs", "APFS"), ("hfsplus", "HFS+"))
            },
        }, indent=2))


if __name__ == "__main__":
    main()
