#!/usr/bin/env python3
"""Exercise the composed project CLI and the HFS+ adapter compatibility tests."""

import argparse
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory

COMPATIBILITY = Path(__file__).resolve().parent
CLI_COMPOSER = Path("tools/compose_hfsplus_cli.py")
CLI_ENV = "DMGEZ_PROJECT_BUILD_CLI"
TARGET_ENV = "CARGO_TARGET_DIR"
HFSPLUS_VERSION = "0.1.0"
HFSPLUS_CORE_PACKAGE = "hfsplus-pomologie-core"
VOLUME_ICON_FIXTURE = "crates/presentation/dmgez-finder/tests/fixtures/apple-iconutil.icns"
DMGEZ_PACKAGES = {
    "dmgez-builder": "crates/build/dmgez-builder",
    "dmgez-config": "crates/foundation/dmgez-config",
    "dmgez-hfsplus": "crates/adapters/dmgez-hfsplus",
    "dmgez-udif": "crates/formats/dmgez-udif",
}
HFSPLUS_PACKAGES = {
    "hfsplus-builder": "crates/filesystem/hfsplus-builder",
    HFSPLUS_CORE_PACKAGE: f"crates/foundation/{HFSPLUS_CORE_PACKAGE}",
    "hfsplus-format": "crates/formats/hfsplus-format",
}


def resolved_packages(root, packages):
    paths = {package: (root / location).resolve(strict=True)
             for package, location in packages.items()}
    for package, path in paths.items():
        if not (path / "Cargo.toml").is_file():
            raise ValueError(f"{package} has no Cargo manifest at {path}")
    return paths


def isolated_manifest(paths):
    template = (COMPATIBILITY / "Cargo.toml.in").read_text(encoding="utf-8")
    path_literals = {name: json.dumps(str(path), ensure_ascii=False)
                     for name, path in paths.items()}
    manifest = re.sub(r"@([a-z-]+)@", lambda match: path_literals[match[1]], template)
    manifest += "\n[patch.crates-io]\n"
    for name in HFSPLUS_PACKAGES:
        manifest += f"{name} = {{ path = {path_literals[name]} }}\n"
    return manifest


def compose_cli(dmgez_source, options, target):
    command = [
        sys.executable, "-B", dmgez_source / CLI_COMPOSER,
        "--no-default-features", "--features", "project-build", "--target-dir", target,
    ]
    if options.hfsplus is not None:
        command.extend(("--hfsplus", options.hfsplus.resolve(strict=True)))
    else:
        command.extend(("--hfsplus-packages", options.hfsplus_packages.resolve(strict=True)))
    if not options.no_default_features:
        command.extend(("--features", "finder-write,parallel"))
    command.extend(feature_arguments(options))
    executable = subprocess.check_output(command, cwd=dmgez_source, encoding="utf-8").strip()
    return Path(executable).resolve(strict=True)


def feature_arguments(options):
    features = []
    if options.lzma:
        features.extend(("--features", "lzma"))
    if options.compression_cache:
        features.extend(("--features", "compression-cache"))
    if options.badge_icons:
        features.extend(("--features", "badge-icons"))
    elif options.volume_icon:
        features.extend(("--features", "volume-icon"))
    return features


def build_project(executable, options):
    command = [executable, "build"]
    if options.dry_run:
        command.append("--dry-run")
    for definition in options.define:
        command.extend(("--define", definition))
    for assignment in options.set:
        command.extend(("--set", assignment))
    if options.overwrite:
        command.append("--overwrite")
    command.extend(("--", options.project))
    return subprocess.run(command).returncode


def test_probe(dmgez_source, options, isolated, executable, target):
    paths = resolved_packages(dmgez_source, DMGEZ_PACKAGES)
    if options.hfsplus is not None:
        paths.update(resolved_packages(options.hfsplus, HFSPLUS_PACKAGES))
    else:
        unpacked = {name: f"{name}-{HFSPLUS_VERSION}" for name in HFSPLUS_PACKAGES}
        paths.update(resolved_packages(options.hfsplus_packages, unpacked))

    shutil.copy2(dmgez_source / "rust-toolchain.toml", isolated / "rust-toolchain.toml")
    for required in ("Cargo.lock", "rustfmt.toml"):
        shutil.copy2(COMPATIBILITY / required, isolated / required)
    for required in ("src", "tests"):
        shutil.copytree(COMPATIBILITY / required, isolated / required)
    if options.volume_icon or options.badge_icons:
        shutil.copy2(dmgez_source / VOLUME_ICON_FIXTURE, isolated / "tests" / "smoke")
    (isolated / "Cargo.toml").write_text(isolated_manifest(paths), encoding="utf-8")

    command = ["cargo", "test", "--offline", "--locked"]
    if options.no_default_features:
        command.append("--no-default-features")
    command.extend(feature_arguments(options))
    if options.include_ignored:
        command.extend(("--", "--include-ignored"))
    environment = os.environ | {CLI_ENV: str(executable), TARGET_ENV: str(target)}
    return subprocess.run(command, cwd=isolated, env=environment).returncode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dmgez", type=Path, required=True, help="dmgez source checkout")
    hfsplus = parser.add_mutually_exclusive_group(required=True)
    hfsplus.add_argument("--hfsplus", type=Path, help="HFS+ source checkout")
    hfsplus.add_argument("--hfsplus-packages", type=Path,
                         help="directory containing unpacked versioned HFS+ packages")
    parser.add_argument("--project", type=Path, help="build this TOML project instead of testing")
    parser.add_argument("--dry-run", action="store_true", help="plan without publishing")
    parser.add_argument("--include-ignored", action="store_true", help="also run opt-in compatibility tests")
    parser.add_argument("--define", metavar="NAME=VALUE", action="append", default=[],
                        help="set a literal project substitution")
    parser.add_argument("--set", metavar="SECTION.FIELD=VALUE", action="append", default=[],
                        help="override a typed scalar setting")
    parser.add_argument("--overwrite", action="store_true",
                        help="authorize replacement in a trusted local output directory")
    parser.add_argument("--no-default-features", action="store_true",
                        help="use the serial profile without Finder store generation")
    parser.add_argument("--lzma", action="store_true", help="enable the optional ULMO codec")
    parser.add_argument("--volume-icon", action="store_true",
                        help="enable volume icons and per-item ICNS icons")
    parser.add_argument("--badge-icons", action="store_true",
                        help="enable volume icons rendered from a PNG badge")
    parser.add_argument("--compression-cache", action="store_true",
                        help="enable project opt-in compression caching")
    options = parser.parse_args()
    if not (sys.platform == "darwin" or sys.platform.startswith("linux")):
        parser.error("the development adapter requires a macOS or Linux host metadata adapter")
    if not options.project and any((options.dry_run, options.define, options.set, options.overwrite)):
        parser.error("--dry-run, --define, --set, and --overwrite require --project")
    if options.project and options.include_ignored:
        parser.error("--include-ignored is available only in test mode")

    dmgez_source = options.dmgez.resolve(strict=True)
    with TemporaryDirectory(prefix="dmgez-hfsplus-build.") as isolated:
        isolated = Path(isolated)
        target = Path(os.environ.get(TARGET_ENV) or isolated / "target").resolve()
        try:
            executable = compose_cli(dmgez_source, options, target)
        except subprocess.CalledProcessError as error:
            return error.returncode
        if options.project:
            # Forward the exit status without printing definition values on failure.
            return build_project(executable, options)
        return test_probe(dmgez_source, options, isolated, executable, target)


if __name__ == "__main__":
    sys.exit(main())
