#!/usr/bin/env python3
"""Qualify dmgez APFS volume selectors against an Apple-generated container."""

import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import re
import subprocess
import sys
from tempfile import TemporaryDirectory


HDIUTIL = "/usr/bin/hdiutil"
DISKUTIL = "/usr/sbin/diskutil"
FSCK = "/sbin/fsck_apfs"
SW_VERS = "/usr/bin/sw_vers"
UNAME = "/usr/bin/uname"
# Apple assigns one volume slot to 64 MiB containers.
IMAGE_SIZE = "1g"
RAW_BYTES = 1 << 30
TIMEOUT = 60
PRIMARY = "Pomologie APFS Primary"
SECONDARY = "Pomologie APFS Secondary"
WHOLE_DEVICE = re.compile(r"/dev/disk\d+")


def run(*argv, check=True):
    return subprocess.run(argv, capture_output=True, check=check, timeout=TIMEOUT)


def plist(*argv):
    return plistlib.loads(run(*argv).stdout)


def native_volumes(source):
    entities = plist(
        HDIUTIL, "attach", "-readwrite", "-nomount", "-nobrowse", "-noautoopen",
        "-plist", source,
    )["system-entities"]
    backing = entities[0]["dev-entry"]
    if not WHOLE_DEVICE.fullmatch(backing):
        raise ValueError(f"unexpected image backing device {backing}")
    try:
        attached = {entry["dev-entry"] for entry in entities}
        physical = plist(DISKUTIL, "info", "-plist", backing)
        container_ref = physical["APFSContainerReference"]
        if (
            physical["VirtualOrPhysical"] != "Virtual"
            or f"/dev/{container_ref}" not in attached
            or not WHOLE_DEVICE.fullmatch(f"/dev/{container_ref}")
            or any("mount-point" in entry for entry in entities)
        ):
            raise ValueError("APFS container did not belong to the attached image")
        container = native_container(container_ref, backing)
        if [volume["Name"] for volume in container["Volumes"]] != [PRIMARY]:
            raise ValueError("fresh APFS fixture did not contain the one expected volume")

        run(DISKUTIL, "apfs", "addVolume", container_ref, "APFS", SECONDARY, "-nomount")
        return {
            volume["Name"]: volume["APFSVolumeUUID"].lower()
            for volume in native_container(container_ref, backing)["Volumes"]
        }
    finally:
        run(HDIUTIL, "detach", backing)
        active = plist(HDIUTIL, "info", "-plist")["images"]
        if any(Path(image["image-path"]).resolve() == source for image in active):
            raise ValueError("the temporary image remained attached")


def native_container(container_ref, backing):
    [container] = plist(DISKUTIL, "apfs", "list", "-plist", container_ref)["Containers"]
    if (
        container["ContainerReference"] != container_ref
        or f"/dev/{container['DesignatedPhysicalStore']}" != backing
        or len(container["PhysicalStores"]) != 1
    ):
        raise ValueError("native metadata did not identify the one expected physical store")
    return container


def check_cli(cli, image, native, *input_options):
    report = json.loads(run(cli, "volumes", image, "--json", *input_options).stdout)
    volumes = report["volumes"]
    assert len(volumes) == len(native), report
    assert {volume["name"]: volume["uuid"].lower() for volume in volumes} == native, report
    assert len({volume["object_id"] for volume in volumes}) == len(native), report
    assert report["volume"]["kind"] == "whole_disk", report

    arguments = (cli, "stat", image, "/", "--filesystem", "apfs", "--json", *input_options)
    ambiguous = run(*arguments, check=False)
    assert ambiguous.returncode and not ambiguous.stdout and b"--volume" in ambiguous.stderr
    for volume in volumes:
        for selector in (volume["uuid"], str(volume["object_id"])):
            stat = json.loads(run(
                *arguments, "--volume", selector, "--xid", str(report["checkpoint_xid"]),
            ).stdout)
            assert stat["kind"] == "directory", stat
            assert stat["apfs_volume"]["uuid"].lower() == volume["uuid"].lower(), stat
            assert stat["apfs_volume"]["object_id"] == volume["object_id"], stat
            assert stat["apfs_volume"]["checkpoint_xid"] == report["checkpoint_xid"], stat
    return report


def qualify(cli, scratch):
    source = scratch / "source.dmg"
    run(
        HDIUTIL, "create", "-size", IMAGE_SIZE, "-type", "UDIF", "-fs", "APFS",
        "-volname", PRIMARY, "-nospotlight", "-layout", "NONE", source,
    )
    native = native_volumes(source)
    assert native.keys() == {PRIMARY, SECONDARY}, native
    assert len(set(native.values())) == len(native), native

    raw_prefix = scratch / "source.raw"
    raw = raw_prefix.with_suffix(".raw.cdr")
    image = scratch / "source-udzo.dmg"
    run(HDIUTIL, "convert", source, "-format", "UDTO", "-o", raw_prefix, "-quiet")
    run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", image, "-quiet")
    assert raw.is_file() and raw.stat().st_size == RAW_BYTES
    assert image.is_file()
    native_result = run(FSCK, "-n", raw)
    native_check = native_result.stdout.decode()
    assert all(f"with UUID {uuid.upper()} appears to be OK" in native_check for uuid in native.values())
    assert f"The container {raw} appears to be OK" in native_check, native_check

    reports = (check_cli(cli, raw, native, "--raw"), check_cli(cli, image, native))
    assert reports[0] == reports[1], reports
    return {
        "macos_version": run(SW_VERS, "-productVersion").stdout.decode().strip(),
        "macos_build": run(SW_VERS, "-buildVersion").stdout.decode().strip(),
        "architecture": run(UNAME, "-m").stdout.decode().strip(),
        "raw_sha256": digest(raw),
        "udzo_sha256": digest(image),
        "native_check": native_check.splitlines(),
        "native_check_stderr": native_result.stderr.decode().splitlines(),
        "volume_discovery": reports[0],
    }


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cli", type=Path, help="path to the composed apfs-read dmgez binary")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("this native qualification requires macOS")
    cli = args.cli.resolve(strict=True)
    try:
        with TemporaryDirectory(prefix="dmgez-apfs-cli-") as root:
            print(json.dumps(qualify(cli, Path(root).resolve()), indent=2))
    except subprocess.CalledProcessError as error:
        print(error.stdout.decode(errors="replace"), file=sys.stderr)
        print(error.stderr.decode(errors="replace"), file=sys.stderr)
        raise


if __name__ == "__main__":
    main()
