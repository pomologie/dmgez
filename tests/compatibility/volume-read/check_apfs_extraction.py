#!/usr/bin/env python3
"""Qualify APFS extraction with an Apple-generated image on macOS."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import sys
from tempfile import TemporaryDirectory

from check_apfs_cli_multivolume import FSCK, SW_VERS, UNAME, digest, run
from check_extract_quarantine import HDIUTIL, MARKER, QUARANTINE, XATTR, attach, extract


DATA = b"native APFS hard-link data\n"
RESOURCE = b"native APFS resource\0fork\n"
EXECUTABLE = b"#!/bin/sh\nexit 0\n"
RESOURCE_ATTRIBUTE = "com.apple.ResourceFork"
IMAGE_SIZE = "128m"
VOLUME_NAME = "Pomologie APFS Extraction"
SECURE_FILE_MODE = 0o600
CLEAN_ALIAS = "/Plain/Shared.txt"
MARKED_PARENT = "/MarkedParent"
MARKED_ALIAS = f"{MARKED_PARENT}/Nested/Shared.txt"
MARKED_OBJECT = "/MarkedFile/Child.txt"


def mark(path):
    run(XATTR, "-w", QUARANTINE, MARKER, path)


def populate(image, mount):
    device = attach(image, mount, "-readwrite")
    try:
        for directory in ("Plain", "MarkedParent", "MarkedFile"):
            (mount / directory).mkdir()
        clean = mount / CLEAN_ALIAS.lstrip("/")
        clean.write_bytes(DATA)
        run(XATTR, "-wx", RESOURCE_ATTRIBUTE, RESOURCE.hex(), clean)
        alias = mount / MARKED_ALIAS.lstrip("/")
        alias.parent.mkdir()
        os.link(clean, alias)
        helper = mount / "Plain" / "helper.sh"
        helper.write_bytes(EXECUTABLE)
        helper.chmod(0o700)
        marked = mount / MARKED_OBJECT.lstrip("/")
        marked.write_bytes(DATA)
        mark(marked)
        mark(mount / MARKED_PARENT.lstrip("/"))
    finally:
        run(HDIUTIL, "detach", device)


def check_native(image, mount):
    device = attach(image, mount, "-readonly")
    try:
        clean = mount / CLEAN_ALIAS.lstrip("/")
        alias = mount / MARKED_ALIAS.lstrip("/")
        assert clean.stat().st_ino == alias.stat().st_ino
        assert clean.read_bytes() == alias.read_bytes() == DATA
        assert bytes.fromhex(run(XATTR, "-px", RESOURCE_ATTRIBUTE, clean).stdout.decode()) == RESOURCE
        marked_parent = mount / MARKED_PARENT.lstrip("/")
        assert run(XATTR, "-p", QUARANTINE, marked_parent).stdout.strip() == MARKER.encode()
        marked = mount / MARKED_OBJECT.lstrip("/")
        assert run(XATTR, "-p", QUARANTINE, marked).stdout.strip() == MARKER.encode()
        assert (mount / "Plain" / "helper.sh").stat().st_mode & stat.S_IXUSR
    finally:
        run(HDIUTIL, "detach", device)


def project(cli, image, path, output, *options, status="complete"):
    result = extract(cli, image, path, output, "--filesystem", "apfs", *options)
    assert result.returncode == (0 if status != "failed" else 1), result
    report = json.loads(result.stdout)
    assert report["status"] == status, report
    return report


def refuses_quarantine(cli, image, path, output, *options):
    report = project(cli, image, path, output, *options, status="failed")
    assert "quarantin" in report["error"].lower(), report
    assert not output.exists()
    return report["error"]


def check_extraction(cli, image, raw, root):
    discovery = json.loads(run(cli, "volumes", image, "--json").stdout)
    [volume] = discovery["volumes"]
    xid = discovery["checkpoint_xid"]
    selection = ("--volume", volume["uuid"], "--xid", xid)

    projected = root / "Projected"
    planned = project(
        cli, image, "/Plain", projected, "--data-only", "--dry-run", *selection, status="planned",
    )
    assert planned["plan"]["files"] == 2 and not projected.exists(), planned
    tree = project(cli, image, "/Plain", projected, "--data-only", *selection)
    assert tree["progress"]["files_published"] == 2, tree
    assert tree["apfs_volume"] == {
        "uuid": volume["uuid"],
        "object_id": volume["object_id"],
        "checkpoint_xid": xid,
    }, tree
    assert (projected / "Shared.txt").read_bytes() == DATA
    assert (projected / "helper.sh").read_bytes() == EXECUTABLE
    assert stat.S_IMODE((projected / "helper.sh").stat().st_mode) == SECURE_FILE_MODE

    resource = root / "Resource.fork"
    report = project(cli, image, CLEAN_ALIAS, resource, "--fork", "resource", *selection)
    assert resource.read_bytes() == RESOURCE, report
    data = root / "Data.fork"
    report = project(cli, raw, CLEAN_ALIAS, data, "--raw", "--fork", "data")
    assert data.read_bytes() == DATA, report
    project(cli, raw, CLEAN_ALIAS, data, "--raw", "--fork", "resource", status="failed")
    assert data.read_bytes() == DATA

    refused = root / "Refused"
    refusals = {
        "selected_alias_ancestor": refuses_quarantine(
            cli, image, MARKED_ALIAS, refused, "--fork", "data", *selection,
        ),
        "selected_file": refuses_quarantine(
            cli, raw, MARKED_OBJECT, refused, "--raw", "--fork", "data",
        ),
        "tree_member": refuses_quarantine(
            cli, image, "/MarkedFile", refused, "--data-only", *selection,
        ),
    }
    mark(image)
    refusals["outer_image"] = refuses_quarantine(
        cli, image, CLEAN_ALIAS, refused, "--fork", "data", "--dry-run", *selection,
    )
    assert run(XATTR, "-p", QUARANTINE, image).stdout.strip() == MARKER.encode()
    return {
        "volume": volume,
        "checkpoint_xid": xid,
        "data_sha256": hashlib.sha256(DATA).hexdigest(),
        "resource_sha256": hashlib.sha256(RESOURCE).hexdigest(),
        "projected_files": tree["progress"]["files_published"],
        "projected_helper_mode": oct(SECURE_FILE_MODE),
        "refusals": refusals,
    }


def qualify(cli, root):
    source = root / "source.dmg"
    image = root / "source-udzo.dmg"
    raw = root / "source.raw.cdr"
    run(
        HDIUTIL, "create", "-size", IMAGE_SIZE, "-fs", "APFS", "-layout", "NONE",
        "-type", "UDIF", "-volname", VOLUME_NAME, "-nospotlight", source,
    )
    populate(source, root / "native-source")
    run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", image, "-quiet")
    run(HDIUTIL, "convert", source, "-format", "UDTO", "-o", root / "source.raw", "-quiet")
    udzo_check = run(HDIUTIL, "verify", image)
    native_check = run(FSCK, "-n", raw)
    fsck_output = native_check.stdout.decode()
    assert f"The container {raw} appears to be OK" in fsck_output
    check_native(image, root / "native-witness")
    raw_hash, image_hash = digest(raw), digest(image)
    evidence = check_extraction(cli, image, raw, root)
    uuid = evidence["volume"]["uuid"].upper()
    assert f"with UUID {uuid} appears to be OK" in fsck_output
    assert digest(raw) == raw_hash and digest(image) == image_hash
    return {
        "macos_version": run(SW_VERS, "-productVersion").stdout.decode().strip(),
        "macos_build": run(SW_VERS, "-buildVersion").stdout.decode().strip(),
        "architecture": run(UNAME, "-m").stdout.decode().strip(),
        "raw_sha256": raw_hash,
        "udzo_sha256": image_hash,
        "fsck_stdout": fsck_output.splitlines(),
        "fsck_stderr": native_check.stderr.decode().splitlines(),
        "udzo_verify_stdout": udzo_check.stdout.decode().splitlines(),
        "udzo_verify_stderr": udzo_check.stderr.decode().splitlines(),
        **evidence,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("the native APFS check requires macOS")
    cli = args.cli.resolve(strict=True)
    os.umask(0o077)
    with TemporaryDirectory(prefix="dmgez-native-apfs-extraction.", dir="/private/tmp") as scratch:
        print(json.dumps(qualify(cli, Path(scratch)), indent=2))


if __name__ == "__main__":
    main()
