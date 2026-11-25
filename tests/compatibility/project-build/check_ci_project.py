#!/usr/bin/env python3
"""Build and ad hoc sign the portable dmg.toml fixture, then verify both images."""

import argparse
import filecmp
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
SIGNED_IMAGE_NAME = "installer-adhoc.dmg"
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


def image_identity(data):
    return {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def build_signed(cli, directory, unsigned):
    report = json.loads(subprocess.check_output([
        cli, "build", PROJECT.name, "--release", "--json", "--set", f"image.output={SIGNED_IMAGE_NAME}",
    ], cwd=directory, encoding="utf-8"))
    image = directory / SIGNED_IMAGE_NAME
    signed = image_identity(image.read_bytes())
    steps = {step["stage"]: step for step in report["steps"]}
    if (
        steps["sign_image"]["backend"] != "rust"
        or steps["sign_image"]["status"] != "passed"
        or steps["verify_image"]["status"] != "passed"
        or steps["publish"]["status"] != "passed"
        or report["unsigned"] != unsigned
        or report["artifact"] != signed
        or signed == unsigned
    ):
        raise RuntimeError(f"unexpected signing release result: {report}")
    return image, signed


def verify_signature(cli, image):
    report = json.loads(subprocess.check_output([
        cli, "sign", "verify", image, "--backend", "rust", "--json",
    ], encoding="utf-8"))
    if (
        report["backend"] != "rust"
        or report["status"] != "verified"
        or report["kind"] != "disk_image"
        or report["assessment"] != "not_performed"
    ):
        raise RuntimeError(f"unexpected portable signature verification result: {report}")


def decode_image(seven_zip, image, raw):
    with raw.open("wb") as stream:
        subprocess.run([seven_zip, "x", "-tdmg", "-so", image], stdout=stream, check=True)
    if raw.stat().st_size != LOGICAL_BYTES:
        raise RuntimeError("7-Zip decoded an unexpected HFS+ image size")


def verify_files(seven_zip, raw):
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
    parser.add_argument("cli", type=Path, help="composed dmgez executable with build, inspect, and signing")
    parser.add_argument("output_dir", type=Path, help="directory for the two verified DMGs")
    parser.add_argument("--seven-zip", default="7z", help="7-Zip executable")
    args = parser.parse_args()
    cli = args.cli.resolve(strict=True)
    seven_zip = shutil.which(args.seven_zip)
    if seven_zip is None:
        parser.error(f"7-Zip executable not found: {args.seven_zip}")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    output = args.output_dir.resolve()
    with tempfile.TemporaryDirectory(prefix="dmgez-ci-", dir=output) as temporary:
        workspace = Path(temporary)
        image = build(cli, workspace / "first", preview=True)
        repeated = build(cli, workspace / "second", preview=False)
        data = image.read_bytes()
        if data != repeated.read_bytes():
            raise RuntimeError("two fresh builds of the project produced different DMG bytes")
        unsigned = image_identity(data)
        verify_container(cli, image)
        raw = workspace / "installer.hfs"
        decode_image(seven_zip, image, raw)
        verify_files(seven_zip, raw)
        signed_image, signed = build_signed(cli, image.parent, unsigned)
        verify_signature(cli, signed_image)
        signed_raw = workspace / "installer-adhoc.hfs"
        decode_image(seven_zip, signed_image, signed_raw)
        if not filecmp.cmp(raw, signed_raw, shallow=False):
            raise RuntimeError("signing changed the independently decoded HFS+ image")
        if image.read_bytes() != data:
            raise RuntimeError("the signing release changed the existing unsigned DMG")
        if sys.platform == "darwin":
            subprocess.run(["/usr/bin/hdiutil", "verify", "-quiet", image], check=True)
            subprocess.run(["/usr/bin/hdiutil", "verify", "-quiet", signed_image], check=True)
            subprocess.run(["/usr/bin/codesign", "--verify", "--strict", signed_image], check=True)
        os.link(image, output / IMAGE_NAME)
        os.link(signed_image, output / SIGNED_IMAGE_NAME)
    print(f"Verified {output / IMAGE_NAME} (SHA-256 {unsigned['sha256']})")
    print(f"Verified {output / SIGNED_IMAGE_NAME} (SHA-256 {signed['sha256']})")


if __name__ == "__main__":
    main()
