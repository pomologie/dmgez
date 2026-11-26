#!/usr/bin/env python3
"""Package a locally composed dmgez CLI and its reviewed dependency notices."""

import argparse
import gzip
import hashlib
import io
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile

from release_notices import NOTICE_DRAFT_HEADER


ROOT = Path(__file__).resolve().parents[1]
CLI_MANIFEST = ROOT / "apps/dmgez-cli/Cargo.toml"
EXAMPLE = ROOT / "examples/offline-cli"
SIGNER = ROOT / "third_party/apple-codesign"
REQUIRED_CAPABILITIES = {"build", "project-release", "signing-rust", "verify-container"}
TARGET_SUFFIXES = {"apple-darwin": "darwin", "unknown-linux-gnu": "linux", "pc-windows-msvc": "win32"}
ARCHITECTURES = {"aarch64", "x86_64"}
TARGETS = sorted(f"{architecture}-{suffix}" for architecture in ARCHITECTURES for suffix in TARGET_SUFFIXES)
MACH_O_64_LE = b"\xcf\xfa\xed\xfe"
ELF_64_LE = b"\x7fELF\x02\x01"
DOS_MAGIC = b"MZ"
PE_MAGIC = b"PE\x00\x00"
ZIP_EPOCH = (1980, 1, 1, 0, 0, 0)
REGULAR_MODE = 0o644
EXECUTABLE_MODE = 0o755
CHECKSUMS = "SHA256SUMS"
BINARY_TIMEOUT_SECONDS = 30


def archive_format(target):
    return ".zip" if target.endswith("-pc-windows-msvc") else ".tar.gz"


def validate_machine(binary, target):
    architecture, suffix = target.split("-", 1)
    system = TARGET_SUFFIXES[suffix]
    if system != sys.platform:
        raise ValueError(f"{target} requires a native {system} packaging host")

    with binary.open("rb") as file:
        header = file.read(64)
        if system == "darwin":
            machine = {"aarch64": 0x0100000C, "x86_64": 0x01000007}[architecture]
            matches = header.startswith(MACH_O_64_LE) and int.from_bytes(header[4:8], "little") == machine
        elif system == "linux":
            machine = {"aarch64": 183, "x86_64": 62}[architecture]
            matches = header.startswith(ELF_64_LE) and int.from_bytes(header[18:20], "little") == machine
        else:
            machine = {"aarch64": 0xAA64, "x86_64": 0x8664}[architecture]
            file.seek(int.from_bytes(header[60:64], "little"))
            signature = file.read(6)
            matches = (header.startswith(DOS_MAGIC) and signature.startswith(PE_MAGIC)
                       and int.from_bytes(signature[4:6], "little") == machine)
    if not matches:
        raise ValueError(f"binary machine or executable format does not match {target}")


def validate_binary(binary, version):
    def run(argument):
        result = subprocess.run(
            [binary, argument], cwd=binary.parent, capture_output=True, encoding="utf-8",
            timeout=BINARY_TIMEOUT_SECONDS, check=True,
        )
        return result.stdout

    if run("--version").strip() != f"dmgez {version}":
        raise ValueError(f"expected dmgez CLI version {version} from this checkout")
    values = dict(line.split(" = ", 1) for line in run("capabilities").splitlines())
    missing = sorted(capability for capability in REQUIRED_CAPABILITIES if values.get(capability) != "compiled")
    if missing:
        raise ValueError(f"composed CLI is missing required capabilities: {', '.join(missing)}")


def release_members(binary_name, binary, notices):
    files = {
        binary_name: (binary, EXECUTABLE_MODE),
        "LICENSE": (ROOT / "LICENSE", REGULAR_MODE),
        "THIRD_PARTY_NOTICES.txt": (notices, REGULAR_MODE),
        "example/dmg.toml": (EXAMPLE / "dmg.toml", REGULAR_MODE),
        "example/hello.txt": (EXAMPLE / "hello.txt", REGULAR_MODE),
    }
    if binary_name == "dmgez":
        files["install.sh"] = (ROOT / "tools/install.sh", EXECUTABLE_MODE)
    covered = (
        ".cargo_vcs_info.json", ".gitattributes", "Cargo.toml", "LICENSE", "README.md",
        "dmgez-profile.patch", "rustfmt.toml", "verify-source.py",
    )
    sources = (SIGNER / "src").glob("**/*.rs")
    for source in (*map(SIGNER.joinpath, covered), *sources):
        name = f"licenses/apple-codesign/{source.relative_to(SIGNER).as_posix()}"
        files[name] = (source, REGULAR_MODE)
    return files


class DigestedReader:
    def __init__(self, file):
        self.file = file
        self.digest = hashlib.sha256()

    def read(self, size=-1):
        data = self.file.read(size)
        self.digest.update(data)
        return data


def write_members(prefix, files, add):
    checksums = []
    for name, (source, mode) in sorted(files.items()):
        with source.open("rb") as file:
            size = os.fstat(file.fileno()).st_size
            reader = DigestedReader(file)
            add(f"{prefix}/{name}", mode, size, reader)
            checksums.append(f"{reader.digest.hexdigest()}  {name}\n")
    contents = "".join(checksums).encode("utf-8")
    add(f"{prefix}/{CHECKSUMS}", REGULAR_MODE, len(contents), io.BytesIO(contents))


def write_tar(output, prefix, files):
    with gzip.GzipFile(fileobj=output, filename="", mode="wb", mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w|", format=tarfile.USTAR_FORMAT) as archive:
            def add(name, mode, size, reader):
                entry = tarfile.TarInfo(name)
                entry.mode = mode
                entry.size = size
                archive.addfile(entry, reader)

            write_members(prefix, files, add)


def write_zip(output, prefix, files):
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_STORED) as archive:
        def add(name, mode, size, reader):
            entry = zipfile.ZipInfo(name, ZIP_EPOCH)
            entry.create_system = 3
            entry.external_attr = (stat.S_IFREG | mode) << 16
            entry.file_size = size
            with archive.open(entry, "w") as member:
                shutil.copyfileobj(reader, member)

        write_members(prefix, files, add)


def package(binary, target, notices, output):
    suffix = archive_format(target)
    if not output.name.endswith(suffix):
        raise ValueError(f"archive output must end in {suffix} for {target}")
    if os.path.lexists(output):
        raise FileExistsError(f"archive already exists: {output}")
    if not binary.is_file():
        raise ValueError("binary must be a regular file")
    if not notices.is_file():
        raise ValueError("third-party notices must be a regular file")

    notice = notices.read_bytes()
    notice_text = notice.decode("utf-8").strip()
    if not notice_text:
        raise ValueError("third-party notices must be nonempty UTF-8 text")
    if notice_text.splitlines()[0] == NOTICE_DRAFT_HEADER:
        raise ValueError("dependency notice draft requires review before packaging")
    with CLI_MANIFEST.open("rb") as file:
        version = tomllib.load(file)["package"]["version"]
    if not re.fullmatch(r"[A-Za-z0-9.+-]+", version):
        raise ValueError("package version cannot be used in an archive path")

    with tempfile.TemporaryDirectory(prefix=".dmgez-package.", dir=output.parent) as temporary:
        stage = Path(temporary).absolute()
        binary_name = "dmgez.exe" if suffix == ".zip" else "dmgez"
        executable = stage / binary_name
        shutil.copyfile(binary, executable)
        executable.chmod(EXECUTABLE_MODE)
        validate_machine(executable, target)
        validate_binary(executable, version)
        notice_path = stage / "notices"
        notice_path.write_bytes(notice)

        files = release_members(binary_name, executable, notice_path)
        archive_path = stage / "archive"
        archive_writer = write_zip if suffix == ".zip" else write_tar
        with archive_path.open("w+b") as archive:
            archive_writer(archive, f"dmgez-{version}-{target}", files)
            archive.seek(0)
            digest = hashlib.file_digest(archive, "sha256").hexdigest()
        os.link(archive_path, output)
    return digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path, help="trusted, host-runnable composed dmgez CLI")
    parser.add_argument("--target", required=True, choices=TARGETS, help="Rust target triple used to build the CLI")
    parser.add_argument("--notices", required=True, type=Path, help="reviewed UTF-8 third-party notices for this build")
    parser.add_argument("--output", required=True, type=Path, help="fresh .tar.gz or .zip archive path")
    args = parser.parse_args()
    try:
        digest = package(args.binary, args.target, args.notices, args.output)
    except subprocess.CalledProcessError as error:
        parser.exit(1, f"dmgez package: {error}\n{error.stderr}")
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        parser.exit(1, f"dmgez package: {error}\n")
    print(f"{digest}  {args.output.name}")


if __name__ == "__main__":
    main()
