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
LOGICAL_BYTES = 4_096_000
PAYLOAD = bytes(range(256)) * 1024
VOLUME_NAME = "Pomologie CI"
PAYLOAD_PATH = f"{VOLUME_NAME}/Documentation/Release Nôtes.bin"
APPLICATIONS_PATH = f"{VOLUME_NAME}/Applications"
APPLICATIONS_TARGET = b"/Applications"
BUILD_REPORT_SCHEMA_VERSION = 1
UDRO = "udro"
UDZO = "udzo"
HDIUTIL = "/usr/bin/hdiutil"


def build(cli, directory, preview, image_format):
    directory.mkdir()
    shutil.copyfile(PROJECT, directory / PROJECT.name)
    (directory / SOURCE_NAME).write_bytes(PAYLOAD)
    image = directory / IMAGE_NAME
    command = (cli, "build", PROJECT.name, "--json", "--set", f"image.format={image_format}")
    expected = {
        "schema_version": BUILD_REPORT_SCHEMA_VERSION,
        "operation": "build",
        "format": image_format,
        "release": "configured_stages_deferred",
        "logical_bytes": str(LOGICAL_BYTES),
    }
    if preview:
        plan = json.loads(subprocess.check_output([*command, "--dry-run"], cwd=directory))
        planned = {
            **expected,
            "mode": "dry_run",
            "filesystem": "hfsplus_unicode",
            "finder": "icon_view",
            "physical_bytes": None,
            "source_payload_bytes": str(len(PAYLOAD)),
        }
        if not planned.items() <= plan.items() or image.exists():
            raise RuntimeError(f"unexpected dry-run result: {plan}")
    report = json.loads(subprocess.check_output(command, cwd=directory))
    published = {
        **expected,
        "mode": "published",
        "physical_bytes": str(image.stat().st_size),
    }
    if not published.items() <= report.items() or not Path(report["output"]).samefile(image):
        raise RuntimeError(f"unexpected unsigned publication result: {report}")
    return image


def verify_container(cli, image, image_format, *, signed=False):
    report = json.loads(subprocess.check_output([cli, "verify", image, "--container", "--json"]))
    runs = report["container"]["runs"]
    encoding_matches = (
        runs["raw"] > 0 and runs["zlib"] == 0 if image_format == UDRO else runs["zlib"] > 0
    )
    if (
        report["outcome"] != "passed"
        or report["coverage"]["payload"] != "passed"
        or report["coverage"]["security"] != "not_checked"
        or (report["container"]["embedded_signature"] is not None) != signed
        or int(report["container"]["logical_bytes"]) != LOGICAL_BYTES
        or not encoding_matches
        or any(runs[codec] != 0 for codec in ("lzfse", "lzma2", "unsupported"))
        or report["checksums"]["data_fork"] != "passed"
        or report["checksums"]["tables"] != ["passed"]
        or report["checksums"]["master"] != "passed"
    ):
        raise RuntimeError(f"unexpected UDIF verification result: {report}")


def image_identity(data):
    return {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def build_signed(cli, directory, unsigned, image_format):
    report = json.loads(subprocess.check_output([
        cli, "build", PROJECT.name, "--release", "--json", "--set", f"image.output={SIGNED_IMAGE_NAME}",
        "--set", f"image.format={image_format}",
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


def verify_with_apple(image, signed_image, raw):
    for candidate in (image, signed_image):
        subprocess.run([HDIUTIL, "verify", "-nocache", "-quiet", candidate], check=True)
        native_raw = raw.with_name(f"apple-{candidate.name}")
        subprocess.run([HDIUTIL, "convert", candidate, "-format", "UDRW", "-o", native_raw, "-quiet"], check=True)
        if not filecmp.cmp(native_raw, raw, shallow=False):
            raise RuntimeError(f"Apple decoded a different HFS+ image from {candidate.name}")
    subprocess.run(["/usr/bin/codesign", "--verify", "--strict", signed_image], check=True)


def verify_native_filesystem(image, mount_point):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    from verify_udif import verify_attachment

    def check_mounted(root):
        if (root / "Documentation" / "Release Nôtes.bin").read_bytes() != PAYLOAD:
            raise RuntimeError("the mounted HFS+ volume returned different payload bytes")
        if os.readlink(os.fsencode(root / "Applications")) != APPLICATIONS_TARGET:
            raise RuntimeError("the mounted HFS+ volume has the wrong Applications link")
        if not (root / ".DS_Store").is_file():
            raise RuntimeError("the mounted HFS+ volume has no Finder store")

    verify_attachment(image, None)
    mount_point.mkdir()
    verify_attachment(image, mount_point, check_mounted)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cli", type=Path, help="composed dmgez executable with build, inspect, and signing")
    parser.add_argument("output_dir", type=Path, help="directory for the two verified DMGs")
    parser.add_argument("--seven-zip", default="7z", help="7-Zip executable")
    parser.add_argument("--format", choices=(UDRO, UDZO), default=UDZO,
                        help="image format to build and verify (default: udzo)")
    parser.add_argument("--native-filesystem", action="store_true",
                        help="also run read-only fsck and a native mounted payload check on macOS")
    args = parser.parse_args()
    if args.native_filesystem and sys.platform != "darwin":
        parser.error("--native-filesystem requires macOS")
    cli = args.cli.resolve(strict=True)
    seven_zip = shutil.which(args.seven_zip)
    if seven_zip is None:
        parser.error(f"7-Zip executable not found: {args.seven_zip}")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    output = args.output_dir.resolve()
    with tempfile.TemporaryDirectory(prefix="dmgez-ci-", dir=output) as temporary:
        workspace = Path(temporary)
        image = build(cli, workspace / "first", preview=True, image_format=args.format)
        repeated = build(cli, workspace / "second", preview=False, image_format=args.format)
        data = image.read_bytes()
        if data != repeated.read_bytes():
            raise RuntimeError("two fresh builds of the project produced different DMG bytes")
        unsigned = image_identity(data)
        verify_container(cli, image, args.format)
        raw = workspace / "installer.hfs"
        decode_image(seven_zip, image, raw)
        verify_files(seven_zip, raw)
        signed_image, signed = build_signed(cli, image.parent, unsigned, args.format)
        verify_container(cli, signed_image, args.format, signed=True)
        verify_signature(cli, signed_image)
        signed_raw = workspace / "installer-adhoc.hfs"
        decode_image(seven_zip, signed_image, signed_raw)
        if not filecmp.cmp(raw, signed_raw, shallow=False):
            raise RuntimeError("signing changed the independently decoded HFS+ image")
        exported = workspace / "exported.hfs"
        subprocess.run([cli, "unwrap-raw", signed_image, "-o", exported], check=True)
        if not filecmp.cmp(signed_raw, exported, shallow=False):
            raise RuntimeError("dmgez and 7-Zip decoded different bytes from the signed DMG")
        if image.read_bytes() != data:
            raise RuntimeError("the signing release changed the existing unsigned DMG")
        if sys.platform == "darwin":
            verify_with_apple(image, signed_image, raw)
        if args.native_filesystem:
            verify_native_filesystem(signed_image, workspace / "mount")
        os.link(image, output / IMAGE_NAME)
        os.link(signed_image, output / SIGNED_IMAGE_NAME)
    print(f"Verified {output / IMAGE_NAME} (SHA-256 {unsigned['sha256']})")
    print(f"Verified {output / SIGNED_IMAGE_NAME} (SHA-256 {signed['sha256']})")


if __name__ == "__main__":
    main()
