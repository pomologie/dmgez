"""Verify the CLI and library from one extracted dmgez Cargo package."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
EXAMPLE = ROOT / "examples/offline-cli"
CONSUMER = ROOT / "tests/compatibility/registry-package/consumer.rs"
PROJECT_SMOKE = ROOT / "tests/compatibility/project-build/check_ci_project.py"
REQUIRED_SOURCES = {
    "LICENSE",
    "README.md",
    "examples/offline-cli/dmg.toml",
    "examples/offline-cli/hello.txt",
    "licenses/apple-codesign/LICENSE",
    "licenses/apple-codesign/source.tar",
    "licenses/hfsplus/LICENSE",
    "src/lib.rs",
    "src/main.rs",
}


def run(cargo, arguments, directory, environment):
    subprocess.run([cargo, *arguments], cwd=directory, env=environment, check=True)


def verify_provenance(source):
    record = json.loads((source / "source-provenance.json").read_text(encoding="utf-8"))
    files = record["files"]
    if missing := REQUIRED_SOURCES - files.keys():
        raise ValueError(f"package provenance is missing {', '.join(sorted(missing))}")
    for relative, expected in files.items():
        path = (source / relative).resolve(strict=True)
        if not path.is_relative_to(source):
            raise ValueError(f"package provenance path escapes the archive: {relative}")
        with path.open("rb") as stream:
            actual = hashlib.file_digest(stream, "sha256").hexdigest()
        if actual != expected:
            raise ValueError(f"packaged source disagrees with provenance: {relative}")


def check(package, cargo, seven_zip, native_filesystem):
    package = package.resolve(strict=True)
    manifest = tomllib.loads((package / "Cargo.toml").read_text(encoding="utf-8"))
    name = manifest["package"]["name"]
    version = manifest["package"]["version"]
    if name != "dmgez":
        raise ValueError(f"expected package dmgez, got {name}")
    if manifest["lib"].get("name", name) != name:
        raise ValueError("expected the dmgez library target")
    if [target["name"] for target in manifest["bin"]] != [name]:
        raise ValueError("expected one dmgez binary target")

    with tempfile.TemporaryDirectory(prefix="dmgez-registry-check-") as temporary:
        scratch = Path(temporary)
        environment = os.environ | {"CARGO_TARGET_DIR": str(scratch / "target")}
        run(cargo, ["package", "--locked", "--offline"], package, environment)
        archive = scratch / "target/package" / f"{name}-{version}.crate"
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        print(f"{digest}  {archive.name}", flush=True)
        unpacked = scratch / "unpacked"
        with tarfile.open(archive, "r:gz") as contents:
            contents.extractall(unpacked, filter="data")
        source = (unpacked / f"{name}-{version}").resolve(strict=True)
        verify_provenance(source)
        installation = scratch / "installation"
        run(cargo, [
            "install", "--locked", "--offline", "--path", source, "--root", installation,
        ], scratch, environment)
        bin_path = installation / "bin"
        environment["PATH"] = os.pathsep.join((str(bin_path), environment["PATH"]))
        executable = shutil.which("dmgez", path=environment["PATH"])
        if executable is None or Path(executable).resolve().parent != bin_path.resolve():
            raise ValueError("Cargo did not install dmgez into its bin directory")
        subprocess.run([executable, "--version"], cwd=scratch, env=environment, check=True)
        for format_name in ("udro", "udzo"):
            command = [
                sys.executable, "-B", PROJECT_SMOKE, executable, scratch / format_name,
                "--format", format_name, "--seven-zip", seven_zip,
            ]
            if native_filesystem:
                command.append("--native-filesystem")
            subprocess.run(command, env=environment, check=True)

        consumer = scratch / "consumer"
        run(cargo, ["new", "--bin", "--vcs", "none", consumer], scratch, environment)
        shutil.copyfile(CONSUMER, consumer / "src/main.rs")
        example = consumer / "example"
        shutil.copytree(EXAMPLE, example)
        run(cargo, ["add", "--offline", "--path", source], consumer, environment)
        run(cargo, [
            "run", "--offline", "--locked", "--", example / "dmg.toml",
        ], consumer, environment)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path, help="standalone dmgez source directory")
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--seven-zip", default="7z")
    parser.add_argument("--native-filesystem", action="store_true")
    args = parser.parse_args()
    if args.native_filesystem and sys.platform != "darwin":
        parser.error("--native-filesystem requires macOS")
    check(args.package, args.cargo, args.seven_zip, args.native_filesystem)


if __name__ == "__main__":
    main()
