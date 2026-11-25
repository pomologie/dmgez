#!/usr/bin/env python3
"""Compare exact extended-attribute changes with native macOS results."""

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
DIRECTORY = "Payload"
FILE = "Entry.bin"
ADDED = "org.pomologie.AddedEmpty"
REMOVED = "org.pomologie.RemovedEmpty"
INLINE = "org.pomologie.Inline"
STREAM = "org.pomologie.Stream"
DIRECTORY_ATTRIBUTE = "org.pomologie.Directory"
INLINE_BEFORE = b"first\0value\xff"
INLINE_AFTER = b"other\0value\xff"
STREAM_BEFORE = b"before\0value\xff" * 693
STREAM_AFTER = b"after!\0value\xff" * 693
FILE_PAYLOAD = b"Unchanged file bytes\n"
RESOURCE_PAYLOAD = b"Unchanged resource fork\0"


def set_xattrs(path, attrs):
    for name, value in attrs.items():
        run(XATTR, "-wx", name, value.hex(), path)


def file_attrs(revised):
    return {
        ADDED if revised else REMOVED: b"",
        INLINE: INLINE_AFTER if revised else INLINE_BEFORE,
        STREAM: STREAM_AFTER if revised else STREAM_BEFORE,
        RESOURCE_ATTRIBUTE: RESOURCE_PAYLOAD,
    }


def create_initial(root):
    directory = root / DIRECTORY
    directory.mkdir()
    entry = directory / FILE
    entry.write_bytes(FILE_PAYLOAD)
    set_xattrs(entry, file_attrs(False))
    set_xattrs(directory, {DIRECTORY_ATTRIBUTE: INLINE_BEFORE})


def edit(root):
    directory = root / DIRECTORY
    entry = directory / FILE
    run(XATTR, "-d", REMOVED, entry)
    set_xattrs(entry, file_attrs(True))
    set_xattrs(directory, {DIRECTORY_ATTRIBUTE: INLINE_AFTER})


def check_native(root, revised):
    directory = root / DIRECTORY
    entry = directory / FILE
    assert entry.read_bytes() == FILE_PAYLOAD
    for path, attrs in (
        (entry, file_attrs(revised)),
        (directory, {DIRECTORY_ATTRIBUTE: INLINE_AFTER if revised else INLINE_BEFORE}),
    ):
        for name, expected in attrs.items():
            result = run(XATTR, "-px", name, path)
            assert bytes.fromhex(result.stdout.decode()) == expected, (path, name)
    absent = REMOVED if revised else ADDED
    assert absent not in run(XATTR, entry).stdout.decode().splitlines()


def comparison(cli, left, right, filesystem, xattrs):
    flags = ("--xattrs",) if xattrs else ()
    result = run(
        cli, "diff", left, right, "--mode", "semantic", "--filesystem", filesystem,
        "--json", *flags, check=False,
    )
    assert result.returncode == int(xattrs), result
    report = json.loads(result.stdout)
    if xattrs:
        assert report["outcome"] == "different", report
        assert report["policy"]["xattrs"], report
        diffs = {
            tuple(change["path"]): (change["fields"], change["xattrs"])
            for change in report["changes"]
        }
        assert diffs == {
            (DIRECTORY,): (["extended_attributes"], [
                {"name": DIRECTORY_ATTRIBUTE, "kind": "changed"},
            ]),
            (DIRECTORY, FILE): (["extended_attributes"], [
                {"name": ADDED, "kind": "added"},
                {"name": INLINE, "kind": "changed"},
                {"name": REMOVED, "kind": "removed"},
                {"name": STREAM, "kind": "changed"},
            ]),
        }, report
    else:
        assert report["outcome"] == "equal" and report["changes"] == [], report
    return report


def qualify(cli, root, filesystem, native_filesystem):
    root.mkdir()
    source = root / "source.dmg"
    left = root / "initial.dmg"
    right = root / "revised.dmg"
    run(
        HDIUTIL, "create", "-size", IMAGE_SIZE, "-fs", native_filesystem, "-layout", "NONE",
        "-type", "UDIF", "-volname", "Pomologie Extended Attributes", "-nospotlight", source,
    )
    mounted(source, root / "initial-write", "-readwrite", create_initial)
    run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", left, "-quiet")
    mounted(source, root / "revised-write", "-readwrite", edit)
    run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", right, "-quiet")
    mounted(left, root / "initial-read", "-readonly", lambda path: check_native(path, False))
    mounted(right, root / "revised-read", "-readonly", lambda path: check_native(path, True))
    checks = {
        path.name: run(HDIUTIL, "verify", path).stdout.decode().splitlines()
        for path in (left, right)
    }
    before = (digest(left), digest(right))
    baseline = comparison(cli, left, right, filesystem, False)
    extended = comparison(cli, left, right, filesystem, True)
    assert before[0] != before[1] and (digest(left), digest(right)) == before
    return {
        "image_digests": dict(zip((left.name, right.name), before, strict=True)),
        "native_udzo_verification": checks,
        "baseline": baseline,
        "extended": extended,
        "stream_value_bytes": len(STREAM_BEFORE),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("the native extended-attribute comparison requires macOS")
    cli = args.cli.resolve(strict=True)
    os.umask(0o077)
    with TemporaryDirectory(prefix="dmgez-native-xattr-diff.", dir="/private/tmp") as scratch:
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
