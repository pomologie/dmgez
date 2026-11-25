#!/usr/bin/env python3
"""Verify a Finder column layout in a read-only mount of a disposable image."""

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory

BASE = Path(__file__).resolve().parent
TESTS = BASE / "tests"
FIXTURE = TESTS / "fixtures" / "finder-column.toml"
ORACLE = TESTS / "smoke" / "column_view_oracle.py"
IMAGE = "column-layout.dmg"
HDIUTIL = "/usr/bin/hdiutil"
OSASCRIPT = "/usr/bin/osascript"
FINDER_COLUMN_VIEW = r"""
on run argv
    set testFolder to POSIX file (item 1 of argv) as alias
    with timeout of 10 seconds
        tell application id "com.apple.finder"
            open testFolder
            repeat 20 times
                repeat with candidate in Finder windows
                    if (target of candidate as alias) = testFolder then
                        set openedView to current view of candidate
                        close candidate
                        return openedView is column view
                    end if
                end repeat
                delay 0.2
            end repeat
        end tell
    end timeout
    error "Finder did not open a window for the test volume"
end run
"""

def check_finder_window(path):
    result = subprocess.check_output(
        [OSASCRIPT, "-e", FINDER_COLUMN_VIEW, path], timeout=15, text=True,
    )
    if result.strip() != "true":
        raise RuntimeError("Finder opened the test volume in a different view")


def check_mounted_image(image, mount, finder_window):
    subprocess.run([HDIUTIL, "verify", image], check=True)
    mount.mkdir()
    subprocess.run([
        HDIUTIL, "attach", "-readonly", "-nobrowse", "-noautoopen", "-mount", "required",
        "-mountpoint", mount, image,
    ], check=True)
    try:
        if not os.statvfs(mount).f_flag & os.ST_RDONLY:
            raise RuntimeError("the test volume did not mount read-only")
        subprocess.run([sys.executable, "-B", ORACLE, mount], check=True)
        if finder_window:
            check_finder_window(mount)
    finally:
        subprocess.run([HDIUTIL, "detach", mount], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dmgez", type=Path, required=True)
    parser.add_argument("--hfsplus", type=Path, required=True)
    parser.add_argument(
        "--finder-window", action="store_true",
        help="open the test volume in Finder and query its current view using Apple Events",
    )
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("the native Finder column check requires macOS")
    with TemporaryDirectory(prefix="dmgez-finder-column-") as scratch:
        root = Path(scratch).resolve()
        project = root / FIXTURE.name
        image = root / IMAGE
        shutil.copy2(FIXTURE, project)
        runner = [
            sys.executable, BASE / "run.py", "--dmgez", args.dmgez, "--hfsplus", args.hfsplus,
            "--project", project,
        ]
        summary = subprocess.check_output([*runner, "--dry-run"], text=True)
        expected = {"finder=ColumnView", "finder_assets=3", "logical_bytes=1048576"}
        if not expected <= set(summary.splitlines()) or image.exists():
            raise RuntimeError(f"unexpected column-view dry run\n{summary}")
        subprocess.run(runner, check=True)
        check_mounted_image(image, root / "mount", args.finder_window)
    print("Verified the default and nested view records on a read-only native mount.")
    if args.finder_window:
        print("Finder opened the test volume in column view.")


if __name__ == "__main__":
    main()
