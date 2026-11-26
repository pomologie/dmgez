#!/usr/bin/env python3
"""Check that a DMG build preserves distinct mounted-HFS+ supplementary names."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


VOLUME = "Name parity"
IMAGE = "names.dmg"
NAME_PREFIX = "note-"
NAME_SUFFIX = ".bin"
FILES = (
    ("half-note.bin", f"{NAME_PREFIX}\U0001d15e{NAME_SUFFIX}", b"half note\n"),
    ("notehead-and-stem.bin", f"{NAME_PREFIX}\U0001d157\U0001d165{NAME_SUFFIX}", b"notehead and stem\n"),
)
PROJECT = f"""schema_version = 1

[image]
name = "{VOLUME}"
output = "{IMAGE}"
format = "udro"
size = "4096k"

[metadata]
policy = "report"
preserve_permissions = false
preserve_ownership = false
preserve_timestamps = false
preserve_extended_attributes = false
preserve_resource_forks = false
preserve_finder_info = false
preserve_hard_links = false

[build]
reproducible = true
source_date_epoch = 1700000000
"""


def verify_catalog(seven_zip, image, raw):
    with raw.open("wb") as output:
        subprocess.run([seven_zip, "x", "-tdmg", "-so", image], stdout=output, check=True)
    listing = subprocess.check_output(
        [seven_zip, "l", "-thfs", "-slt", "-sccUTF-8", raw], encoding="utf-8",
    ).replace("\r\n", "\n")
    sizes = {}
    for block in listing.split("\n\n"):
        fields = dict(line.split(" = ", 1) for line in block.splitlines() if " = " in line)
        if "Path" in fields and "Size" in fields:
            sizes[fields["Path"].replace("\\", "/")] = fields["Size"]
    expected = {f"{VOLUME}/{name}": str(len(payload)) for _, name, payload in FILES}
    if not expected.items() <= sizes.items():
        raise RuntimeError(f"7-Zip did not list the exact names and payload sizes:\n{listing}")
    # Select both entries with an ASCII pattern so the host cannot normalize
    # an argument before 7-Zip receives it.
    pattern = f"{VOLUME}/{NAME_PREFIX}*{NAME_SUFFIX}".replace("/", os.sep)
    actual = subprocess.check_output([seven_zip, "x", "-thfs", "-so", raw, pattern])
    first, second = (payload for _, _, payload in FILES)
    if actual not in (first + second, second + first):
        raise RuntimeError("7-Zip returned different supplementary-name payloads")


def verify_native(image, mount_point):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    from verify_udif import verify_attachment

    def check_mounted(root):
        for _, name, payload in FILES:
            if (root / name).read_bytes() != payload:
                raise RuntimeError(f"Native HFS+ returned different bytes for {name!r}")

    mount_point.mkdir()
    verify_attachment(image, mount_point, check_mounted)


def check(cli, seven_zip, native_filesystem):
    with tempfile.TemporaryDirectory(prefix="dmgez-name-parity-") as temporary:
        workspace = Path(temporary)
        manifest = workspace / "dmg.toml"
        contents = []
        for source, name, payload in FILES:
            (workspace / source).write_bytes(payload)
            destination = json.dumps(f"/{name}", ensure_ascii=False)
            contents.append(f'\n[[contents]]\nsource = "{source}"\ndestination = {destination}\n')
        manifest.write_text(PROJECT + "".join(contents), encoding="utf-8", newline="\n")
        image = workspace / IMAGE
        build = [cli, "build", manifest, "--json"]
        before = set(workspace.iterdir())
        plan = json.loads(subprocess.check_output([*build, "--dry-run"], encoding="utf-8"))
        if plan["mode"] != "dry_run" or set(workspace.iterdir()) != before:
            raise RuntimeError("The project dry run did not preserve the output directory")
        subprocess.run(build, check=True)
        subprocess.run([cli, "verify", image, "--container"], check=True)
        verify_catalog(seven_zip, image, workspace / "names.hfs")
        if native_filesystem:
            verify_native(image, workspace / "mount")
    print("Verified distinct supplementary HFS+ names", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cli", type=Path)
    parser.add_argument("--seven-zip", default="7z")
    parser.add_argument("--native-filesystem", action="store_true")
    args = parser.parse_args()
    if args.native_filesystem and sys.platform != "darwin":
        parser.error("--native-filesystem requires macOS")
    seven_zip = shutil.which(args.seven_zip)
    if seven_zip is None:
        parser.error(f"7-Zip executable not found: {args.seven_zip}")
    check(args.cli.resolve(strict=True), seven_zip, args.native_filesystem)


if __name__ == "__main__":
    main()
