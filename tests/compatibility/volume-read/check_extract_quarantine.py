#!/usr/bin/env python3
"""Qualify extraction against a disposable native HFS+ image on macOS."""

import argparse
import hashlib
import json
import os
import plistlib
from pathlib import Path
import stat
import subprocess
import sys
from tempfile import TemporaryDirectory

HDIUTIL = "/usr/bin/hdiutil"
XATTR = "/usr/bin/xattr"
QUARANTINE = "com.apple.quarantine"
MARKER = "0081;00000000;dmgez-extraction-witness;"
SOURCE_TEXT = b"independent macOS HFS+ data fork\n"
EXECUTABLE_TEXT = b"#!/bin/sh\nexit 0\n"

def attach(image, mount, mode):
    mount.mkdir()
    result = subprocess.check_output([
        HDIUTIL, "attach", mode, "-nobrowse", "-noautoopen", "-mount", "required",
        "-mountpoint", mount, "-plist", image,
    ])
    mounted, = [
        entity["dev-entry"] for entity in plistlib.loads(result)["system-entities"]
        if entity.get("mount-point") == str(mount)
    ]
    return mounted


def extract(cli, source, path, output, *flags):
    return subprocess.run([
        cli, "extract", source, path, "--output", output, "--json", *flags,
    ], capture_output=True, text=True)


def refuse(result, output):
    assert result.returncode != 0 and "quarantin" in result.stderr.lower(), result
    assert not output.exists(), result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("the native HFS+ check requires macOS")
    cli = args.cli.resolve(strict=True)
    os.umask(0o077)

    with TemporaryDirectory(prefix="dmgez-native-extraction.", dir="/private/tmp") as scratch:
        root = Path(scratch)
        writable = root / "fixture-rw.dmg"
        image = root / "fixture.dmg"
        subprocess.run([
            HDIUTIL, "create", "-size", "64m", "-fs", "HFS+", "-layout", "NONE", "-type", "UDIF",
            "-volname", "dmgez Extract Witness", writable,
        ], check=True)

        mount = root / "native-source"
        device = attach(writable, mount, "-readwrite")
        try:
            plain = mount / "Plain"
            plain.mkdir()
            (plain / "About.txt").write_bytes(SOURCE_TEXT)
            executable = plain / "helper.sh"
            executable.write_bytes(EXECUTABLE_TEXT)
            executable.chmod(0o700)
            marked = mount / "Marked"
            marked.mkdir()
            (marked / "Child.txt").write_bytes(SOURCE_TEXT)
            subprocess.run([XATTR, "-w", QUARANTINE, MARKER, marked], check=True)
        finally:
            subprocess.run([HDIUTIL, "detach", device], check=True)

        subprocess.run([HDIUTIL, "convert", writable, "-format", "UDZO", "-o", image], check=True)
        subprocess.run([HDIUTIL, "verify", image], check=True)
        witness = root / "native-witness"
        device = attach(image, witness, "-readonly")
        try:
            assert (witness / "Plain" / "About.txt").read_bytes() == SOURCE_TEXT
            assert (witness / "Plain" / "helper.sh").read_bytes() == EXECUTABLE_TEXT
            assert stat.S_IMODE((witness / "Plain" / "helper.sh").stat().st_mode) & stat.S_IXUSR
            assert subprocess.check_output([XATTR, "-p", QUARANTINE, witness / "Marked"]).strip() == MARKER.encode()
        finally:
            subprocess.run([HDIUTIL, "detach", device], check=True)

        original = hashlib.sha256(image.read_bytes()).digest()
        output = root / "Projected"
        dry = extract(cli, image, "/Plain", output, "--data-only", "--dry-run")
        assert dry.returncode == 0 and not output.exists(), dry
        result = extract(cli, image, "/Plain", output, "--data-only")
        assert result.returncode == 0, result
        assert json.loads(result.stdout)["status"] == "complete", result
        assert (output / "About.txt").read_bytes() == SOURCE_TEXT
        assert (output / "helper.sh").read_bytes() == EXECUTABLE_TEXT
        assert stat.S_IMODE((output / "helper.sh").stat().st_mode) == 0o600
        absent = root / "Refused"
        refuse(extract(cli, image, "/Marked/Child.txt", absent, "--fork", "data"), absent)
        refuse(extract(cli, image, "/", absent, "--data-only"), absent)
        subprocess.run([XATTR, "-w", QUARANTINE, MARKER, image], check=True)
        refuse(extract(cli, image, "/Plain/About.txt", absent, "--fork", "data", "--dry-run"), absent)
        assert subprocess.check_output([XATTR, "-p", QUARANTINE, image]).strip() == MARKER.encode()
        assert hashlib.sha256(image.read_bytes()).digest() == original
        print("Native HFS+ content matched; projection kept 0600 and refused inner and outer quarantine.")


if __name__ == "__main__":
    main()
