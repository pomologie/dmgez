#!/usr/bin/env python3
"""Run the isolated HFS+ adapter against explicit worktree or package paths."""

import argparse
import json
import re
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory

COMPATIBILITY = Path(__file__).resolve().parent
HFSPLUS_VERSION = "0.1.0"
DMGEZ_PACKAGES = {
    "dmgez-builder": "crates/services/dmgez-builder",
    "dmgez-config": "crates/foundation/dmgez-config",
    "dmgez-core": "crates/foundation/dmgez-core",
    "dmgez-udif": "crates/formats/dmgez-udif",
}
HFSPLUS_PACKAGES = {
    "hfsplus-builder": "crates/filesystem/hfsplus-builder",
    "hfsplus-core": "crates/foundation/hfsplus-core",
    "hfsplus-format": "crates/formats/hfsplus-format",
}


def resolved_packages(root, packages):
    paths = {package: (root / location).resolve(strict=True)
             for package, location in packages.items()}
    for package, path in paths.items():
        if not (path / "Cargo.toml").is_file():
            raise ValueError(f"{package} has no Cargo manifest at {path}")
    return paths


def isolated_manifest(paths, packaged):
    template = (COMPATIBILITY / "Cargo.toml.in").read_text(encoding="utf-8")
    path_literals = {name: json.dumps(str(path), ensure_ascii=False)
                     for name, path in paths.items()}
    manifest = re.sub(r"@([a-z-]+)@", lambda match: path_literals[match[1]], template)
    if packaged:
        manifest += "\n[patch.crates-io]\n"
        for name in ("hfsplus-core", "hfsplus-format"):
            manifest += f"{name} = {{ path = {path_literals[name]} }}\n"
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dmgez", type=Path, required=True, help="dmgez source checkout")
    hfsplus = parser.add_mutually_exclusive_group(required=True)
    hfsplus.add_argument("--hfsplus", type=Path, help="HFS+ source checkout")
    hfsplus.add_argument("--hfsplus-packages", type=Path,
                         help="directory containing unpacked versioned HFS+ packages")
    parser.add_argument("--project", type=Path, help="build this TOML project instead of testing")
    parser.add_argument("--no-default-features", action="store_true")
    options = parser.parse_args()
    if not (sys.platform == "darwin" or sys.platform.startswith("linux")):
        parser.error("the development adapter requires a macOS or Linux host metadata adapter")

    dmgez_source = options.dmgez.resolve(strict=True)
    paths = resolved_packages(dmgez_source, DMGEZ_PACKAGES)
    if options.hfsplus is not None:
        paths.update(resolved_packages(options.hfsplus, HFSPLUS_PACKAGES))
    else:
        unpacked = {name: f"{name}-{HFSPLUS_VERSION}" for name in HFSPLUS_PACKAGES}
        paths.update(resolved_packages(options.hfsplus_packages, unpacked))

    command = ["cargo", "run" if options.project else "test", "--offline", "--locked"]
    command.append("--no-default-features" if options.no_default_features else "--all-features")
    if options.project:
        command.extend(["--", str(options.project.resolve(strict=True))])
    with TemporaryDirectory(prefix="dmgez-hfsplus-build.") as isolated:
        isolated = Path(isolated)
        shutil.copy2(dmgez_source / "rust-toolchain.toml", isolated / "rust-toolchain.toml")
        for required in ("Cargo.lock", "rustfmt.toml"):
            shutil.copy2(COMPATIBILITY / required, isolated / required)
        for required in ("src", "tests"):
            shutil.copytree(COMPATIBILITY / required, isolated / required)
        (isolated / "Cargo.toml").write_text(isolated_manifest(paths, options.hfsplus_packages is not None), encoding="utf-8")
        subprocess.run(command, cwd=isolated, check=True)


if __name__ == "__main__":
    main()
