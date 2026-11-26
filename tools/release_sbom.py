#!/usr/bin/env python3
"""Generate a CycloneDX build inventory for a packaged dmgez CLI."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
import tarfile
import tomllib
from urllib.parse import quote
import zipfile

from compose_filesystem_cli import CLI_PACKAGE
from package_cli import TARGETS, archive_format
from release_notices import FIELD_SEPARATOR, LOCK_SHA256_FIELD, PACKAGE_HEADING, notice_digests, write_new


SCHEMA = "https://cyclonedx.org/schema/bom-1.6.schema.json"
NOTICE_NAME = "THIRD_PARTY_NOTICES.txt"
PROFILE_FIELDS = ("Default features", "Additional features", "License tool")
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
SHA256 = re.compile(r"[0-9a-f]{64}")


def notice_inventory(text, target, lock_hash):
    identity = notice_digests(text, target)
    if identity is None:
        raise ValueError("SBOM generation requires machine-generated, reviewed dependency notices")
    header, separator, body = text.partition(f"\n\n{PACKAGE_HEADING}\n")
    if not separator:
        raise ValueError("dependency notice lacks the built-package inventory")
    fields = {}
    for line in header.splitlines()[1:]:
        name, separator, value = line.partition(FIELD_SEPARATOR)
        if not separator or name in fields:
            raise ValueError(f"invalid dependency notice field: {line}")
        fields[name] = value
    if fields.get(LOCK_SHA256_FIELD) != lock_hash:
        raise ValueError("composed Cargo.lock digest differs from the reviewed dependency notice")
    packages = {}
    for line in body.split("\n\n", 1)[0].splitlines():
        label, _, license_expression = line.partition(" | ")
        key = tuple(label.split())
        if len(key) != 2 or not license_expression.strip() or key in packages:
            raise ValueError(f"invalid dependency notice package: {line}")
        packages[key] = license_expression
    if not packages:
        raise ValueError("dependency notice built-package inventory is empty")
    return identity, fields, packages


def locked_components(lock, licenses):
    components = {}
    for package in lock["package"]:
        name, version = key = package["name"], package["version"]
        if key not in licenses:
            continue
        if key in components:
            raise ValueError(f"ambiguous locked package {name} {version}")
        component = {
            "type": "library", "name": name, "version": version,
            "bom-ref": f"cargo:{name}@{version}",
            "licenses": [{"expression": licenses[key]}],
        }
        source = package.get("source")
        if source == CRATES_IO:
            checksum = package.get("checksum", "")
            if not SHA256.fullmatch(checksum):
                raise ValueError(f"locked registry package {name} {version} has no SHA-256")
            component["purl"] = f"pkg:cargo/{quote(name, safe='')}@{quote(version, safe='')}"
            component["hashes"] = [{"alg": "SHA-256", "content": checksum}]
        elif source is not None:
            raise ValueError(f"unrecognized Cargo source for {name} {version}")
        components[key] = component
    if missing := licenses.keys() - components.keys():
        names = ", ".join(f"{name} {version}" for name, version in sorted(missing))
        raise ValueError(f"built packages absent from composed Cargo.lock: {names}")
    return components


def archive_inventory(archive, target, lock_bytes):
    suffix = archive_format(target)
    prefix = archive.name.removesuffix(suffix)
    if prefix == archive.name or not prefix.startswith("dmgez-") or not prefix.endswith(f"-{target}"):
        raise ValueError(f"archive filename must be dmgez-<version>-{target}{suffix}")
    lock_hash = hashlib.sha256(lock_bytes).hexdigest()
    binary_name = "dmgez.exe" if suffix == ".zip" else "dmgez"
    with archive.open("rb") as stream:
        archive_hash = hashlib.file_digest(stream, "sha256").hexdigest()
        stream.seek(0)
        if suffix == ".zip":
            with zipfile.ZipFile(stream) as reader:
                notice = reader.read(f"{prefix}/{NOTICE_NAME}")
                with reader.open(f"{prefix}/{binary_name}") as binary:
                    binary_hash = hashlib.file_digest(binary, "sha256").hexdigest()
        else:
            with tarfile.open(fileobj=stream, mode="r:gz") as reader:
                notice_entry = reader.getmember(f"{prefix}/{NOTICE_NAME}")
                binary_entry = reader.getmember(f"{prefix}/{binary_name}")
                if not notice_entry.isfile() or not binary_entry.isfile():
                    raise ValueError("archive CLI and dependency notice must be regular files")
                with reader.extractfile(notice_entry) as member:
                    notice = member.read()
                with reader.extractfile(binary_entry) as binary:
                    binary_hash = hashlib.file_digest(binary, "sha256").hexdigest()
    (expected_binary, signer_hash), fields, licenses = notice_inventory(notice.decode("utf-8"), target, lock_hash)
    if binary_hash != expected_binary:
        raise ValueError("archived CLI digest differs from the reviewed dependency notice")
    components = locked_components(tomllib.loads(lock_bytes.decode("utf-8")), licenses)
    version = prefix.removeprefix("dmgez-").removesuffix(f"-{target}")
    application = components.pop((CLI_PACKAGE, version), None)
    if application is None:
        raise ValueError("archived CLI version is missing from the built-package inventory")
    application.update(type="application", name="dmgez", hashes=[{"alg": "SHA-256", "content": binary_hash}])
    properties = {
        "target": target, "archive": archive.name, "archive-sha256": archive_hash,
        "notices-sha256": hashlib.sha256(notice).hexdigest(), "composed-lock-sha256": lock_hash,
        "signer-source-sha256": signer_hash,
        **{field.lower().replace(" ", "-"): fields[field] for field in PROFILE_FIELDS},
    }
    return {
        "$schema": SCHEMA, "bomFormat": "CycloneDX", "specVersion": "1.6", "version": 1,
        "metadata": {
            "lifecycles": [{"phase": "build"}], "component": application,
            "properties": [{"name": f"dmgez:{key}", "value": value} for key, value in properties.items()],
        },
        "components": [components[key] for key in sorted(components)],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--lockfile", type=Path, required=True, help="Cargo.lock from the composed CLI build")
    parser.add_argument("--output", type=Path, required=True, help="new CycloneDX JSON file")
    args = parser.parse_args()
    try:
        bom = archive_inventory(args.archive, args.target, args.lockfile.read_bytes())
        write_new(args.output, (json.dumps(bom, indent=2) + "\n").encode("utf-8"))
    except (OSError, ValueError, KeyError, tarfile.TarError, zipfile.BadZipFile) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
