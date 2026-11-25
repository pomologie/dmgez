#!/usr/bin/env python3
"""Reconstruct and verify the pinned apple-codesign source profile."""

import hashlib
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile


ARCHIVE_ROOT = "apple-codesign-0.29.0"
ARCHIVE_SHA256 = "f24e9ebdb70a2aee3ca1cea217009fb50776955f0d7678c31d22e48c1524667f"
PATCH = "dmgez-profile.patch"
FILES = (
    ".cargo_vcs_info.json",
    "Cargo.toml",
    "LICENSE",
    "src/code_directory.rs",
    "src/code_requirement.rs",
    "src/cryptography.rs",
    "src/dmg.rs",
    "src/embedded_signature.rs",
    "src/embedded_signature_builder.rs",
    "src/environment_constraints.rs",
    "src/error.rs",
    "src/lib.rs",
    "src/plist_der.rs",
)


def verify(archive_path, package):
    with archive_path.open("rb") as archive_file:
        digest = hashlib.file_digest(archive_file, "sha256").hexdigest()
        if digest != ARCHIVE_SHA256:
            raise ValueError("archive SHA-256 differs from the pinned release")
        archive_file.seek(0)
        with tempfile.TemporaryDirectory() as temporary:
            extracted = Path(temporary)
            with tarfile.open(fileobj=archive_file, mode="r:gz") as archive:
                for name in FILES:
                    member = archive.getmember(f"{ARCHIVE_ROOT}/{name}")
                    if not member.isfile():
                        raise ValueError(f"archive source is not a file: {name}")
                    path = extracted / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    with archive.extractfile(member) as stream:
                        path.write_bytes(stream.read())

            subprocess.run(
                ["patch", "-s", "-f", "-F", "0", "-p1", "-i", str(package / PATCH)],
                cwd=extracted,
                check=True,
            )
            expected_sources = {name for name in FILES if name.startswith("src/")}
            actual_sources = {
                path.relative_to(package).as_posix()
                for path in (package / "src").rglob("*")
                if path.is_file()
            }
            if expected_sources != actual_sources:
                raise ValueError("vendored source file list differs")
            for name in FILES:
                if (extracted / name).read_bytes() != (package / name).read_bytes():
                    raise ValueError(f"vendored file differs from the reconstructed source: {name}")


def main():
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {Path(sys.argv[0]).name} UPSTREAM_CRATE_ARCHIVE")
    package = Path(__file__).resolve().parent
    try:
        verify(Path(sys.argv[1]), package)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
    print("Verified vendored apple-codesign against the pinned archive and profile patch")


if __name__ == "__main__":
    main()
