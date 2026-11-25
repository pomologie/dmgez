#!/usr/bin/env python3
"""Exercise offline packaging with a native composed CLI and reviewed notices."""

import argparse
import hashlib
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tarfile
import tempfile
import zipfile

import package_cli


PACKAGER = Path(package_cli.__file__).resolve()
CHECKSUMS = "SHA256SUMS"


def run(*arguments, **options):
    return subprocess.run(arguments, capture_output=True, text=True, check=False, **options)


def expect_success(result):
    assert result.returncode == 0, f"{result.args}\n{result.stdout}\n{result.stderr}"
    return result.stdout


def digest(file):
    with file.open("rb") as content:
        return hashlib.file_digest(content, "sha256").hexdigest()


def extract(path, destination, windows):
    if windows:
        with zipfile.ZipFile(path) as archive:
            names = archive.namelist()
            confirm_member_paths(names)
            assert archive.testzip() is None
            archive.extractall(destination)
    else:
        with tarfile.open(path, "r:gz") as archive:
            names = archive.getnames()
            confirm_member_paths(names)
            assert all(member.isfile() for member in archive)
            if hasattr(tarfile, "data_filter"):
                archive.extractall(destination, filter="data")
            else:
                archive.extractall(destination)
    return destination / PurePosixPath(names[0]).parts[0]


def confirm_member_paths(names):
    assert names and len(set(names)) == len(names)
    parts = [PurePosixPath(name) for name in names]
    assert all(not name.is_absolute() and len(name.parts) > 1 and ".." not in name.parts for name in parts)
    assert len({name.parts[0] for name in parts}) == 1


def verify_archive(root, binary, notices, *, windows):
    checksums = root / CHECKSUMS
    expected = {path.relative_to(root).as_posix() for path in root.rglob("*") if path.is_file()}
    expected.remove(CHECKSUMS)
    declared = set()
    for line in checksums.read_text(encoding="utf-8").splitlines():
        checksum, name = line.split("  ", 1)
        assert name in expected and name not in declared, name
        assert digest(root / name) == checksum, name
        declared.add(name)
    assert declared == expected

    executable = root / ("dmgez.exe" if windows else "dmgez")
    assert digest(executable) == digest(binary)
    assert (root / "LICENSE").read_bytes() == (package_cli.ROOT / "LICENSE").read_bytes()
    assert digest(root / "THIRD_PARTY_NOTICES.txt") == digest(notices)
    if not windows:
        assert os.access(executable, os.X_OK)
        assert os.access(root / "install.sh", os.X_OK)
    capabilities = expect_success(run(executable, "capabilities"))
    if "signing-rust = compiled" in capabilities:
        assert (root / "licenses/apple-codesign/LICENSE").is_file()
        assert (root / "licenses/apple-codesign/src/lib.rs").is_file()
    if not windows:
        install_directory = root.parent / "installed"
        expect_success(run(root / "install.sh", "--bin-dir", install_directory))
        assert expect_success(run(install_directory / "dmgez", "--version")) == expect_success(run(executable, "--version"))
        assert digest(install_directory / "dmgez") == digest(executable)

    manifest = root / "example" / "dmg.toml"
    image = manifest.with_name("example.dmg")
    expect_success(run(executable, "build", manifest, "--dry-run"))
    assert not image.exists()
    expect_success(run(executable, "build", manifest))
    assert image.is_file()
    expect_success(run(executable, "verify", image, "--container"))


def main():
    if not __debug__:
        raise SystemExit("run the packaging test with Python assertions enabled")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--target", required=True, choices=package_cli.TARGETS)
    parser.add_argument("--notices", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    notices = args.notices.resolve(strict=True)
    suffix = package_cli.archive_format(args.target)
    windows = suffix == ".zip"

    with tempfile.TemporaryDirectory(prefix="dmgez-package-test.") as temporary:
        root = Path(temporary)
        first = root / f"first{suffix}"
        second = root / f"second{suffix}"
        command = [sys.executable, "-B", PACKAGER, "--binary", binary, "--target", args.target, "--notices", notices]
        reported = expect_success(run(*command, "--output", first))
        assert reported.strip() == f"{digest(first)}  {first.name}"
        expect_success(run(*command, "--output", second))
        assert first.read_bytes() == second.read_bytes()
        result = run(*command, "--output", first)
        assert result.returncode == 1 and "already exists" in result.stderr
        assert first.read_bytes() == second.read_bytes()

        extracted = extract(first, root / "extracted", windows)
        version = expect_success(run(binary, "--version")).split()[1]
        assert extracted.name == f"dmgez-{version}-{args.target}"
        verify_archive(extracted, binary, notices, windows=windows)
    print("Deterministic packaging, extracted checksums, installation and example DMG verification passed")


if __name__ == "__main__":
    main()
