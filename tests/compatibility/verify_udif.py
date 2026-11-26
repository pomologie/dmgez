#!/usr/bin/env python3
"""Exercise both directions of UDIF interoperability with Apple and 7-Zip."""
import argparse
import hashlib
import json
import pathlib
import plistlib
import re
import subprocess
import sys
import tempfile

HDIUTIL = "/usr/bin/hdiutil"
APPLE_RAW_FORMAT = "UDTO"
EMPTY_BYTE = 0
REFERENCE_CHUNK_BYTES = 1 << 20
FIXTURES = pathlib.Path(__file__).resolve().parents[1] / "fixtures"


def run(*args):
    subprocess.run(args, check=True)


def same_bytes(actual, expected):
    if actual.read_bytes() != expected.read_bytes():
        raise RuntimeError(f"raw bytes differ for {actual.name} and {expected.name}")


def export_with_apple(image, output):
    run(HDIUTIL, "convert", str(image), "-format", APPLE_RAW_FORMAT, "-o", str(output), "-quiet")
    return output.with_suffix(".cdr")


def export_with_dmgez(cli, image, output):
    run(cli, "verify", str(image))
    run(cli, "unwrap-raw", str(image), "-o", str(output))
    return output


def exercise_control_runs(cli, native, work):
    tiny = native / "tiny.raw"
    for name in ("zero-run", "ignore-run", "comment-run"):
        image = native / f"{name}.dmg"
        same_bytes(export_with_dmgez(cli, image, work / f"from-dmgez-{name}.raw"), tiny)
        same_bytes(export_with_apple(image, work / f"from-apple-{name}"), tiny)

    name = "hybrid-UDZO"
    image = native / f"{name}.dmg"
    apple = export_with_apple(image, work / "from-apple-hybrid").read_bytes()
    dmgez = export_with_dmgez(cli, image, work / "from-dmgez-hybrid.raw").read_bytes()
    manifest = json.loads((native / f"{name}.json").read_text())
    if hashlib.sha256(dmgez).hexdigest() != manifest["decoded_sha256"]:
        raise RuntimeError("dmgez export does not match the independently decoded APM image")
    if not dmgez.startswith(apple) or any(byte != EMPTY_BYTE for byte in dmgez[len(apple):]):
        raise RuntimeError("dmgez export differs from the Apple prefix and its declared zero tail")


def exercise_reference_readers(cli, reference_reader, native, work):
    tiny = native / "tiny.raw"
    for mode in ("UDRO", "UDZO"):
        run(reference_reader, native / f"tiny-{mode}.dmg", tiny)

    # The catalog's apple-dmg oracle counts stored bytes for zero runs.
    # Keep this fixture to one zlib run and one raw fallback run.
    mixed = (native / "mixed.raw").read_bytes()
    raw = work / "reference.raw"
    raw.write_bytes(mixed[REFERENCE_CHUNK_BYTES:3 * REFERENCE_CHUNK_BYTES])
    for mode in ("udro", "udzo"):
        image = work / f"reference-{mode}.dmg"
        run(cli, "wrap-raw", str(raw), "--format", mode, "-o", str(image))
        report = json.loads(subprocess.check_output([
            cli, "verify", str(image), "--container", "--json",
        ]))
        runs = report["container"]["runs"]
        expected = (2, 0) if mode == "udro" else (1, 1)
        if (runs["raw"], runs["zlib"]) != expected:
            raise RuntimeError(f"unexpected {mode} reference run types: {runs}")
        same_bytes(export_with_apple(image, work / f"apple-reference-{mode}"), raw)
        run(reference_reader, image, raw)


def exercise_hfs(cli, work):
    native = work / "native-hfs.dmg"
    run(HDIUTIL, "create", "-size", "8m", "-fs", "HFS+", "-volname", "Pomologie Fixture",
        "-layout", "NONE", str(native))
    raw = export_with_apple(native, work / "source-hfs")
    output = work / "dmgez-hfs.dmg"
    run(cli, "wrap-raw", str(raw), "-o", str(output))
    run(HDIUTIL, "verify", str(output))
    verify_attachment(output, None)
    mounted = work / "mount"
    mounted.mkdir()
    verify_attachment(output, mounted)


def verify_attachment(image, mount_point, check_mounted=None):
    options = ["-nomount"] if mount_point is None else ["-mountpoint", str(mount_point), "-mount", "required"]
    result = subprocess.check_output([HDIUTIL, "attach", "-readonly", "-nobrowse", "-noautoopen",
                                      "-plist", *options, str(image)])
    entities = plistlib.loads(result)["system-entities"]
    devices = [entry["dev-entry"] for entry in entities
               if re.fullmatch(r"/dev/disk[0-9]+", entry.get("dev-entry", ""))]
    try:
        if len(devices) != 1:
            raise RuntimeError("Apple did not return exactly one new whole-image device")
        if mount_point is None:
            run("/sbin/fsck_hfs", "-fn", devices[0])
        else:
            mounted_paths = {pathlib.Path(entry["mount-point"]).resolve()
                             for entry in entities if "mount-point" in entry}
            if mount_point.resolve() not in mounted_paths:
                raise RuntimeError("Apple did not mount the disposable volume at the requested path")
            if check_mounted is None:
                run("/usr/sbin/diskutil", "info", str(mount_point))
            else:
                check_mounted(mount_point)
    finally:
        for device in devices:
            run(HDIUTIL, "detach", device)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cli", type=pathlib.Path)
    parser.add_argument("--seven-zip", default="7z")
    parser.add_argument("--reference-readers", type=pathlib.Path,
                        help="standalone apple-dmg and udif fixture adapter")
    parser.add_argument("--attach-hfs", action="store_true")
    args = parser.parse_args()
    cli = str(args.cli.resolve(strict=True))
    run("/usr/bin/sw_vers")
    run("/usr/bin/uname", "-m")
    with tempfile.TemporaryDirectory(prefix="dmgez-native-") as temporary:
        work = pathlib.Path(temporary)
        native = work / "native"
        run(sys.executable, str(FIXTURES / "generate_udif.py"), str(native))
        for sample in ("tiny", "mixed", "zero"):
            raw = native / f"{sample}.raw"
            for mode in ("UDRO", "UDZO"):
                oracle_image = native / f"{sample}-{mode}.dmg"
                result = export_with_dmgez(cli, oracle_image, work / f"from-apple-{sample}-{mode}.raw")
                same_bytes(result, raw)
                emitted = work / f"dmgez-{sample}-{mode}.dmg"
                run(cli, "wrap-raw", str(raw), "--format", mode.lower(), "-o", str(emitted))
                run(HDIUTIL, "verify", str(emitted))
                same_bytes(export_with_apple(emitted, work / f"from-dmgez-{sample}-{mode}"), raw)
                if subprocess.check_output([args.seven_zip, "x", "-so", str(emitted)]) != raw.read_bytes():
                    raise RuntimeError(f"7-Zip exported different bytes for {emitted.name}")
        exercise_control_runs(cli, native, work)
        if args.reference_readers:
            exercise_reference_readers(cli, args.reference_readers.resolve(strict=True), native, work)
        if args.attach_hfs:
            exercise_hfs(cli, work)


if __name__ == "__main__":
    main()
