#!/usr/bin/env python3
"""Qualify APFS Unicode path matching on disposable native macOS images."""

import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import struct
import sys
from tempfile import TemporaryDirectory
import unicodedata

from check_apfs_cli_multivolume import FSCK, HDIUTIL, SW_VERS, UNAME, digest, run
from check_native_paths_diff import rename_case
from check_semantic_diff import diff, mounted


IMAGE_SIZE = "128m"
FOLDER = "Study-\u00c9"
FILE = "Stra\u00dfe-\u00c9.txt"
STABLE = "Stable.bin"
ORIGINAL_DATA = b"before\n"
REVISED_DATA = b"edited\n"
STABLE_DATA = b"same contents\n"
COLLISION_FOLDER = "Collision"
COLLISION_LEFT = "entry-234770544d80.txt"
COLLISION_RIGHT = "entry-7b8525cba68d.txt"
COLLISION_DATA = {COLLISION_LEFT: b"left\n", COLLISION_RIGHT: b"peer\n"}
COLLISION_REVISED = b"edit\n"
COLLISION_HASH = 0x22FBE7
POST_UNICODE9 = "\U00011F04.txt"
POST_UNICODE9_DATA = b"Unicode 15 Kawi letter\n"
NATIVE = ("--paths", "native")
EXIT_EQUAL = 0
EXIT_DIFFERENT = 1
EXIT_ERROR = 2

BLOCK_BYTES = 4096
CHECKSUM_BYTES = 8
CHECKSUM_MODULUS = 0xFFFFFFFF
OBJECT_TYPE_OFFSET = 24
OBJECT_TYPE_MASK = 0xFFFF
MAGIC_OFFSET = 32
BLOCK_SIZE_OFFSET = 36
VOLUME_FLAGS_OFFSET = 56
NODE_HEADER_OFFSET = 32
NODE_DATA_OFFSET = 56
NODE_TOC_SLOT_BYTES = 8
OBJECT_B_TREE_ROOT = 2
OBJECT_B_TREE_NODE = 3
OBJECT_VOLUME_SUPERBLOCK = 13
FILESYSTEM_TREE = 14
NODE_LEAF = 1 << 1
NODE_FIXED_KEYS = 1 << 2
INCOMPAT_CASE_INSENSITIVE = 1 << 0
INCOMPAT_NORMALIZATION_INSENSITIVE = 1 << 3
DREC_KIND = 9
DREC_KIND_SHIFT = 60
DREC_ID_MASK = (1 << DREC_KIND_SHIFT) - 1
DREC_HASH_SHIFT = 10
DREC_LENGTH_MASK = (1 << DREC_HASH_SHIFT) - 1
DREC_HEADER_BYTES = struct.calcsize("<QI")


@dataclass(frozen=True)
class Profile:
    filesystem: str
    flag: int
    policy: str
    folder: str
    file: str


PROFILES = {
    "folded": Profile(
        "APFS",
        INCOMPAT_CASE_INSENSITIVE,
        "apfs_unicode9_folded",
        "study-e\u0301",
        "STRASSE-E\u0301.txt",
    ),
    "normalized": Profile(
        "Case-sensitive APFS",
        INCOMPAT_NORMALIZATION_INSENSITIVE,
        "apfs_unicode9_normalized",
        "Study-E\u0301",
        "Stra\u00dfe-E\u0301.txt",
    ),
}


def create_initial(root):
    folder = root / FOLDER
    folder.mkdir()
    (folder / FILE).write_bytes(ORIGINAL_DATA)
    (folder / STABLE).write_bytes(STABLE_DATA)
    collision = root / COLLISION_FOLDER
    collision.mkdir()
    for name, data in COLLISION_DATA.items():
        (collision / name).write_bytes(data)


def edit_names(root, profile):
    folder = root / profile.folder
    rename_case(root / FOLDER, folder)
    rename_case(folder / FILE, folder / profile.file)
    (folder / profile.file).write_bytes(REVISED_DATA)


def native_facts(root, profile, revised, collision_edited, has_new_scalar):
    folder_name, file_name = (profile.folder, profile.file) if revised else (FOLDER, FILE)
    folder = root / folder_name
    file = folder / file_name
    assert folder_name in os.listdir(root), os.listdir(root)
    assert sorted(os.listdir(folder)) == sorted((file_name, STABLE))
    assert file.read_bytes() == (REVISED_DATA if revised else ORIGINAL_DATA)
    assert (folder / STABLE).read_bytes() == STABLE_DATA

    form = "NFC" if revised else "NFD"
    canonical_folder = unicodedata.normalize(form, folder_name)
    canonical_file = unicodedata.normalize(form, file_name)
    assert canonical_folder != folder_name and canonical_file != file_name
    canonical = root / canonical_folder / canonical_file
    assert canonical.stat().st_ino == file.stat().st_ino
    case_alias = root / folder_name.casefold() / file_name.casefold()
    folded = profile.flag == INCOMPAT_CASE_INSENSITIVE
    if folded:
        assert case_alias.stat().st_ino == file.stat().st_ino
    else:
        assert not case_alias.exists()

    collision = root / COLLISION_FOLDER
    assert sorted(os.listdir(collision)) == sorted(COLLISION_DATA)
    assert (collision / COLLISION_LEFT).stat().st_ino != (collision / COLLISION_RIGHT).stat().st_ino
    for name, data in COLLISION_DATA.items():
        expected = COLLISION_REVISED if collision_edited and name == COLLISION_LEFT else data
        assert (collision / name).read_bytes() == expected

    if has_new_scalar:
        assert POST_UNICODE9 in os.listdir(root)
        assert (root / POST_UNICODE9).read_bytes() == POST_UNICODE9_DATA
    return {
        "stored_path": [folder_name, file_name],
        "canonical_alias": [canonical_folder, canonical_file],
        "casefold_alias": [folder_name.casefold(), file_name.casefold()],
        "casefold_alias_resolves": folded,
        "collision_directory_inode": collision.stat().st_ino,
        "collision_inodes": {name: (collision / name).stat().st_ino for name in COLLISION_DATA},
        "later_unicode_name": POST_UNICODE9 if has_new_scalar else None,
    }


def checksummed(block):
    low = high = 0
    values = memoryview(block)[CHECKSUM_BYTES:]
    for (word,) in struct.iter_unpack("<I", values):
        low = (low + word) % CHECKSUM_MODULUS
        high = (high + low) % CHECKSUM_MODULUS
    check_low = CHECKSUM_MODULUS - (low + high) % CHECKSUM_MODULUS
    check_high = CHECKSUM_MODULUS - (low + check_low) % CHECKSUM_MODULUS
    return struct.unpack_from("<II", block) == (check_low, check_high)


def collision_records(block, parent_inode):
    flags, level, count, toc_offset, toc_length = struct.unpack_from(
        "<HHIHH", block, NODE_HEADER_OFFSET,
    )
    if level or not flags & NODE_LEAF or flags & NODE_FIXED_KEYS:
        return
    toc = NODE_DATA_OFFSET + toc_offset
    keys = toc + toc_length
    if keys > BLOCK_BYTES or count > toc_length // NODE_TOC_SLOT_BYTES:
        return
    for index in range(count):
        offset, length = struct.unpack_from("<HH", block, toc + index * NODE_TOC_SLOT_BYTES)
        start = keys + offset
        end = start + length
        if length <= DREC_HEADER_BYTES or end > BLOCK_BYTES:
            continue
        record, packed = struct.unpack_from("<QI", block, start)
        if record >> DREC_KIND_SHIFT != DREC_KIND or record & DREC_ID_MASK != parent_inode:
            continue
        if packed & DREC_LENGTH_MASK != length - DREC_HEADER_BYTES or block[end - 1] != 0:
            continue
        for name in COLLISION_DATA:
            if block[start + DREC_HEADER_BYTES:end - 1] == name.encode("ascii"):
                yield name, packed >> DREC_HASH_SHIFT


def scan_native_keys(raw, parent_inode, profile):
    flags = set()
    hashes = {name: set() for name in COLLISION_DATA}
    with raw.open("rb") as source:
        header = source.read(BLOCK_BYTES)
        assert header[MAGIC_OFFSET:MAGIC_OFFSET + 4] == b"NXSB"
        assert struct.unpack_from("<I", header, BLOCK_SIZE_OFFSET)[0] == BLOCK_BYTES
        for block in iter(lambda: source.read(BLOCK_BYTES), b""):
            assert len(block) == BLOCK_BYTES
            kind, subtype = struct.unpack_from("<II", block, OBJECT_TYPE_OFFSET)
            kind &= OBJECT_TYPE_MASK
            if kind == OBJECT_VOLUME_SUPERBLOCK:
                if block[MAGIC_OFFSET:MAGIC_OFFSET + 4] == b"APSB" and checksummed(block):
                    flags.add(struct.unpack_from("<Q", block, VOLUME_FLAGS_OFFSET)[0])
            elif kind in (OBJECT_B_TREE_ROOT, OBJECT_B_TREE_NODE) and subtype == FILESYSTEM_TREE:
                records = list(collision_records(block, parent_inode))
                if records and checksummed(block):
                    for name, value in records:
                        hashes[name].add(value)
    assert flags == {profile.flag}, flags
    assert all(values == {COLLISION_HASH} for values in hashes.values()), hashes
    return {
        "incompatible_flags": hex(profile.flag),
        "stored_hashes": {name: hex(COLLISION_HASH) for name in hashes},
    }


def expect_paths(cli, initial, revised, collision, later, profile):
    exact = diff(cli, initial, revised, EXIT_DIFFERENT, "--paths", "exact")
    removed = [(FOLDER,), (FOLDER, FILE), (FOLDER, STABLE)]
    added = [(profile.folder,), (profile.folder, profile.file), (profile.folder, STABLE)]
    expected = {**dict.fromkeys(removed, "removed"), **dict.fromkeys(added, "added")}
    assert exact["outcome"] == "different", exact
    assert len(exact["changes"]) == len(expected), exact
    assert {tuple(item["path"]): item["kind"] for item in exact["changes"]} == expected, exact

    native = diff(cli, initial, revised, EXIT_DIFFERENT, *NATIVE)
    assert native["policy"]["paths"] == "native", native
    assert native["policy"]["native_name_profile"] == profile.policy, native
    assert native["changes"] == [{
        "path": [FOLDER, FILE],
        "kind": "changed",
        "fields": ["data_fork"],
        "right_path": [profile.folder, profile.file],
    }], native
    assert native["equivalent_path_spellings"] == [
        {"left": [FOLDER], "right": [profile.folder]},
        {"left": [FOLDER, STABLE], "right": [profile.folder, STABLE]},
    ], native
    assert diff(cli, initial, revised, EXIT_DIFFERENT, *NATIVE) == native

    collision_report = diff(cli, revised, collision, EXIT_DIFFERENT, *NATIVE)
    assert collision_report["changes"] == [{
        "path": [COLLISION_FOLDER, COLLISION_LEFT],
        "kind": "changed",
        "fields": ["data_fork"],
    }], collision_report

    refused = run(
        cli, "diff", later, later, "--mode", "semantic", "--filesystem", "apfs", "--json",
        *NATIVE, check=False,
    )
    diagnostic = refused.stderr.decode().strip()
    assert refused.returncode == EXIT_ERROR and not refused.stdout, refused
    assert "could not obtain a native name key for the left path" in diagnostic, diagnostic
    assert "U+11F04" in diagnostic, diagnostic
    exact_later = diff(cli, later, later, EXIT_EQUAL, "--paths", "exact")
    assert exact_later["outcome"] == "equal" and exact_later["changes"] == [], exact_later
    return {
        "exact": exact,
        "native": native,
        "collision_control": collision_report,
        "later_unicode": {
            "exit": refused.returncode,
            "diagnostic": diagnostic,
            "exact_outcome": exact_later["outcome"],
        },
    }


def qualify(cli, root, profile):
    root.mkdir()
    source = root / "source.dmg"
    raw_prefix = root / "initial.raw"
    raw = root / "initial.raw.cdr"
    initial = root / "initial.dmg"
    revised = root / "revised.dmg"
    collision = root / "collision.dmg"
    later = root / "later-unicode.dmg"
    run(
        HDIUTIL, "create", "-size", IMAGE_SIZE, "-fs", profile.filesystem, "-layout", "NONE",
        "-type", "UDIF", "-volname", "Pomologie APFS Native Paths", "-nospotlight", source,
    )
    mounted(source, root / "write-initial", "-readwrite", create_initial)
    run(HDIUTIL, "convert", source, "-format", "UDTO", "-o", raw_prefix, "-quiet")
    run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", initial, "-quiet")
    mounted(source, root / "write-revised", "-readwrite", lambda path: edit_names(path, profile))
    run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", revised, "-quiet")
    mounted(
        source, root / "write-collision", "-readwrite",
        lambda path: (path / COLLISION_FOLDER / COLLISION_LEFT).write_bytes(COLLISION_REVISED),
    )
    run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", collision, "-quiet")
    mounted(
        source, root / "write-later-unicode", "-readwrite",
        lambda path: (path / POST_UNICODE9).write_bytes(POST_UNICODE9_DATA),
    )
    run(HDIUTIL, "convert", source, "-format", "UDZO", "-o", later, "-quiet")
    images = (initial, revised, collision, later)
    inputs = (raw, *images)
    before = {path.name: digest(path) for path in inputs}
    assert len(set(before.values())) == len(inputs)

    facts = {}
    for index, image in enumerate(images):
        run(HDIUTIL, "verify", image)
        mounted(
            image, root / f"read-{image.stem}", "-readonly",
            lambda path: facts.update({
                image.name: native_facts(path, profile, index > 0, index > 1, index > 2),
            }),
        )
    fsck = run(FSCK, "-n", raw)
    assert b"appears to be OK" in fsck.stdout, fsck
    keys = scan_native_keys(raw, facts[initial.name]["collision_directory_inode"], profile)
    comparison = expect_paths(cli, initial, revised, collision, later, profile) if cli else {}
    assert {path.name: digest(path) for path in inputs} == before
    return {
        "inputs_sha256": before,
        "inputs_unchanged": True,
        "all_udzo_verifications_passed": True,
        "fsck_exit": fsck.returncode,
        "native_keys": keys,
        "native_facts": facts,
        **comparison,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--cli", type=Path)
    mode.add_argument("--native-only", action="store_true")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("the APFS native path oracle requires macOS")
    cli = args.cli.resolve(strict=True) if args.cli is not None else None
    os.umask(0o077)
    with TemporaryDirectory(prefix="dmgez-apfs-native-paths.", dir="/private/tmp") as scratch:
        root = Path(scratch)
        print(json.dumps({
            "macos_version": run(SW_VERS, "-productVersion").stdout.decode().strip(),
            "macos_build": run(SW_VERS, "-buildVersion").stdout.decode().strip(),
            "architecture": run(UNAME, "-m").stdout.decode().strip(),
            "profiles": {label: qualify(cli, root / label, profile) for label, profile in PROFILES.items()},
        }, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
