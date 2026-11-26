#!/usr/bin/env python3
"""Run a filesystem read adapter against an explicit source or package set."""

import argparse
import hashlib
import json
import re
import shutil
import subprocess
from pathlib import Path
from tempfile import TemporaryDirectory

RUNNER = Path(__file__).resolve().parent
ENGINE_VERSION = "0.1.0"
HFSPLUS_CORE_PACKAGE = "hfsplus-pomologie-core"
HFSPLUS_ADAPTER_PACKAGE = "dmgez-hfsplus"
HFSPLUS_ENGINE = "hfsplus"
APFS_ENGINE = "apfs"
APFS_ADAPTER_PACKAGE = "dmgez-apfs"
HFSX_FEATURE = "hfsx-fixture"
DMGEZ_PACKAGES = {
    "dmgez-operations": "crates/runtime/dmgez-operations",
    "dmgez-udif": "crates/formats/dmgez-udif",
    "dmgez-vfs": "crates/foundation/dmgez-vfs",
}
ENGINE_PACKAGES = {
    HFSPLUS_ENGINE: {
        HFSPLUS_CORE_PACKAGE: f"crates/foundation/{HFSPLUS_CORE_PACKAGE}",
        "hfsplus-format": "crates/formats/hfsplus-format",
        "hfsplus-builder": "crates/filesystem/hfsplus-builder",
        "hfsplus-reader": "crates/filesystem/hfsplus-reader",
    },
    APFS_ENGINE: {
        "libapfs-core": "crates/foundation/apfs-core",
        "apfs-format": "crates/formats/apfs-format",
        "apfs-container": "crates/container/apfs-container",
        "apfs-reader": "crates/filesystem/apfs-reader",
    },
}
PACKAGE_PATCHES = {
    HFSPLUS_ENGINE: (HFSPLUS_CORE_PACKAGE, "hfsplus-format", "hfsplus-reader"),
    APFS_ENGINE: tuple(ENGINE_PACKAGES[APFS_ENGINE]),
}
ADAPTER_PACKAGES = {
    HFSPLUS_ENGINE: HFSPLUS_ADAPTER_PACKAGE,
    APFS_ENGINE: APFS_ADAPTER_PACKAGE,
}


def resolve_packages(root, locations):
    packages = {}
    for name, relative in locations.items():
        package = (root / relative).resolve()
        if not (package / "Cargo.toml").is_file():
            raise ValueError(f"{name} has no Cargo manifest at {package}")
        packages[name] = package
    return packages


def manifest(recipe, packages, patches):
    template = (recipe / "Cargo.toml.in").read_text(encoding="utf-8")
    paths = {name: json.dumps(str(path), ensure_ascii=False)
             for name, path in packages.items()}
    expanded = re.sub(r"@([a-z-]+)@", lambda match: paths[match[1]], template)
    if patches:
        expanded += "\n[patch.crates-io]\n"
        expanded += "".join(f"{name} = {{ path = {paths[name]} }}\n" for name in patches)
    return expanded


def install_fixtures(indices, packages, destination):
    destination.mkdir()
    for index in indices:
        expected = json.loads(index.read_text(encoding="utf-8"))
        for package, fixtures in expected.items():
            for name, digest in fixtures.items():
                source = packages[package] / "tests" / "fixtures" / name
                data = source.read_bytes()
                if hashlib.sha256(data).hexdigest() != digest:
                    raise ValueError(f"{package} fixture {name} has an unexpected SHA-256 digest")
                (destination / name).write_bytes(data)


def run(dmgez, engine, source, packaged, hfsx):
    locations = ENGINE_PACKAGES[engine]
    if packaged:
        locations = {name: f"{name}-{ENGINE_VERSION}" for name in locations}
    recipe = RUNNER / engine
    packages = resolve_packages(dmgez, DMGEZ_PACKAGES)
    packages.update(resolve_packages(source, locations))
    adapter = ADAPTER_PACKAGES[engine]
    packages.update(resolve_packages(dmgez, {adapter: f"crates/adapters/{adapter}"}))
    patches = PACKAGE_PATCHES[engine]
    fixtures = [recipe / "fixtures.json"]
    command = ["cargo", "test", "--offline", "--locked"]
    if hfsx:
        fixtures.append(recipe / "fixtures-hfsx.json")
        command.extend(["--features", HFSX_FEATURE])
    with TemporaryDirectory(prefix="dmgez-volume-read.") as temporary:
        isolated = Path(temporary)
        for name in ("rust-toolchain.toml", "rustfmt.toml"):
            shutil.copy2(dmgez / name, isolated / name)
        shutil.copy2(recipe / "Cargo.lock", isolated / "Cargo.lock")
        for name in ("src", "tests"):
            shutil.copytree(recipe / name, isolated / name)
        install_fixtures(fixtures, packages, isolated / "tests" / "fixtures")
        (isolated / "Cargo.toml").write_text(manifest(recipe, packages, patches), encoding="utf-8")
        return subprocess.run(command, cwd=isolated).returncode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dmgez", required=True, type=Path, help="dmgez source checkout")
    parser.add_argument(f"--{HFSX_FEATURE}", dest="hfsx", action="store_true",
                        help="test the pinned native case-sensitive HFSX fixture")
    sources = parser.add_mutually_exclusive_group(required=True)
    choices = {}
    for engine in ENGINE_PACKAGES:
        if not (RUNNER / engine / "Cargo.lock").is_file():
            continue
        choices[engine] = (engine, False)
        sources.add_argument(f"--{engine}", type=Path, help=f"{engine} source checkout")
        if engine in PACKAGE_PATCHES:
            choices[f"{engine}_packages"] = (engine, True)
            sources.add_argument(f"--{engine}-packages", type=Path,
                                 help=f"directory containing unpacked {engine} {ENGINE_VERSION} packages")
    options = vars(parser.parse_args())
    for option, (engine, packaged) in choices.items():
        source = options[option]
        if source is not None:
            if options["hfsx"] and engine != HFSPLUS_ENGINE:
                parser.error(f"--{HFSX_FEATURE} requires --hfsplus or --hfsplus-packages")
            try:
                return run(
                    options["dmgez"].resolve(), engine, source.resolve(), packaged, options["hfsx"],
                )
            except (OSError, ValueError) as error:
                parser.error(str(error))


if __name__ == "__main__":
    raise SystemExit(main())
