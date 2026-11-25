#!/usr/bin/env python3
"""Build the portable dmg.toml fixture and verify its DMG independently."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unicodedata


PROJECT = Path(__file__).resolve().parent / "ci" / "dmg.toml"
SOURCE_NAME = "release nôtes.bin"
IMAGE_NAME = "installer.dmg"
LOGICAL_BYTES = 4 * 1024 * 1024
PAYLOAD = bytes(range(256)) * 1024
VOLUME_NAME = "Pomologie CI"
PAYLOAD_PATH = f"{VOLUME_NAME}/Documentation/Release Nôtes.bin"
APPLICATIONS_PATH = f"{VOLUME_NAME}/Applications"
APPLICATIONS_TARGET = b"/Applications"


def build(cli, directory, preview):
    directory.mkdir()
    shutil.copyfile(PROJECT, directory / PROJECT.name)
    (directory / SOURCE_NAME).write_bytes(PAYLOAD)
    image = directory / IMAGE_NAME
    command = (cli, "build", PROJECT.name)
    if preview:
        plan = subprocess.check_output([*command, "--dry-run"], cwd=directory, encoding="utf-8")
        expected = {
            "mode=dry-run", "format=Udzo", "filesystem=HfsplusUnicode", "finder=IconView",
            f"logical_bytes={LOGICAL_BYTES}", f"source_payload_bytes={len(PAYLOAD)}",
        }
        if expected.difference(plan.splitlines()) or image.exists():
            raise RuntimeError(f"unexpected dry-run result:\n{plan}")
    subprocess.run(command, cwd=directory, check=True)
    return image


def verify_container(cli, image):
    report = json.loads(subprocess.check_output([cli, "verify", image, "--container", "--json"]))
    if (
        report["outcome"] != "passed"
        or report["coverage"]["payload"] != "passed"
        or int(report["container"]["logical_bytes"]) != LOGICAL_BYTES
        or report["container"]["runs"]["zlib"] == 0
        or report["checksums"]["data_fork"] != "passed"
        or report["checksums"]["tables"] != ["passed"]
        or report["checksums"]["master"] != "passed"
    ):
        raise RuntimeError(f"unexpected UDIF verification result: {report}")


def verify_files(seven_zip, image, raw):
    with raw.open("wb") as stream:
        subprocess.run([seven_zip, "x", "-tdmg", "-so", image], stdout=stream, check=True)
    if raw.stat().st_size != LOGICAL_BYTES:
        raise RuntimeError("7-Zip decoded an unexpected HFS+ image size")

    listing = subprocess.check_output([seven_zip, "l", "-thfs", "-slt", "-sccUTF-8", raw])
    listing = unicodedata.normalize("NFC", listing.decode("utf-8")).replace("\\", "/")
    paths = {
        line.removeprefix("Path = ") for line in listing.splitlines() if line.startswith("Path = ")
    }
    expected = {PAYLOAD_PATH, APPLICATIONS_PATH, f"{VOLUME_NAME}/.DS_Store"}
    if not expected.issubset(paths):
        raise RuntimeError(f"7-Zip could not list the expected HFS+ entries:\n{listing}")

    # The listing checks the exact Unicode name. Use an ASCII selection pattern
    # for extraction so host argument normalization cannot change the match.
    payload_pattern = f"{VOLUME_NAME}/Documentation/Release N*.bin".replace("/", os.sep)
    extract = [seven_zip, "x", "-thfs", "-so", raw]
    if subprocess.check_output([*extract, payload_pattern]) != PAYLOAD:
        raise RuntimeError("7-Zip read back different bytes for the nested payload")
    if subprocess.check_output([*extract, APPLICATIONS_PATH.replace("/", os.sep)]) != APPLICATIONS_TARGET:
        raise RuntimeError("7-Zip read back a different target for the Applications symlink")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cli", type=Path, help="composed dmgez executable with build and inspect")
    parser.add_argument("output_dir", type=Path, help="directory for the verified installer.dmg")
    parser.add_argument("--seven-zip", default="7z", help="7-Zip executable")
    args = parser.parse_args()
    cli = args.cli.resolve(strict=True)
    seven_zip = shutil.which(args.seven_zip)
    if seven_zip is None:
        parser.error(f"7-Zip executable not found: {args.seven_zip}")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    output = args.output_dir.resolve() / IMAGE_NAME
    with tempfile.TemporaryDirectory(prefix="dmgez-ci-", dir=output.parent) as temporary:
        workspace = Path(temporary)
        image = build(cli, workspace / "first", preview=True)
        repeated = build(cli, workspace / "second", preview=False)
        data = image.read_bytes()
        if data != repeated.read_bytes():
            raise RuntimeError("two fresh builds of the project produced different DMG bytes")
        verify_container(cli, image)
        verify_files(seven_zip, image, workspace / "installer.hfs")
        if sys.platform == "darwin":
            subprocess.run(["/usr/bin/hdiutil", "verify", "-quiet", image], check=True)
        os.link(image, output)
    print(f"Verified {output} (SHA-256 {hashlib.sha256(data).hexdigest()})")


if __name__ == "__main__":
    main()
