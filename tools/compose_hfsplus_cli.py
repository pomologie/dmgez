#!/usr/bin/env python3
"""Build dmgez with locally supplied, versioned Pomologie HFS+ packages."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory
import tomllib


ROOT = Path(__file__).resolve().parents[1]
CLI = Path("apps/dmgez-cli")
CLI_PACKAGE = "dmgez-cli"
CLI_BINARY = "dmgez"
CLI_TEST = "project_build_cli"
VERIFY_TEST = "hfs_verify_cli"
READ_TEST = "volume_cli"
EXTRACT_TEST = "extract_cli"
TOOLCHAIN = "rust-toolchain.toml"
MANIFEST = "Cargo.toml"
LOCK = "Cargo.lock"
HFSPLUS_VERSION = "0.1.0"
HFSPLUS_BUILDER = "hfsplus-builder"
HFSPLUS_BUILDER_SOURCE = "crates/filesystem/hfsplus-builder"
HFSPLUS_READER = "hfsplus-reader"
HFSPLUS_READER_SOURCE = "crates/filesystem/hfsplus-reader"
HFSPLUS_FIXTURES_ENV = "DMGEZ_HFSPLUS_FIXTURES"
HFSPLUS_PACKAGES = {
    "hfsplus-format": "crates/formats/hfsplus-format",
    "hfsplus-pomologie-core": "crates/foundation/hfsplus-pomologie-core",
}
ADAPTER = "dmgez-hfsplus"
READ_ADAPTER = "dmgez-hfsplus-read"
READ_FEATURE = "hfsplus-read"
VERIFY_FEATURE = "hfsplus-verify"
EXTRACT_FEATURE = "hfsplus-extract"
READ_PROFILES = (READ_FEATURE, EXTRACT_FEATURE)
SOURCE_DIRS = ("apps", "crates", "tests")
FEATURE_ADDITIONS = {
    "project-build": ["dep:dmgez-builder", f"dep:{ADAPTER}", "dep:dmgez-config"],
    "finder-write": ["dmgez-builder?/finder-write", f"{ADAPTER}?/finder-write"],
    "volume-icon": ["finder-write", f"{ADAPTER}?/volume-icon"],
    "badge-icons": ["volume-icon", f"{ADAPTER}?/badge-icons"],
}
FEATURE_FORWARDING = {
    "build": ["project-build", "finder-write"],
    "parallel": [f"{ADAPTER}?/parallel"],
    "lzfse": [f"{ADAPTER}?/lzfse"],
    "lzma": [f"{ADAPTER}?/lzma"],
}


def load_toml(path):
    with path.open("rb") as stream:
        return tomllib.load(stream)


def has_cli_feature(options, name):
    names = {name, f"{CLI_PACKAGE}/{name}"}
    return any(names.intersection(argument.replace(",", " ").split()) for argument in options.features)


def resolve_hfsplus(source, packages, reader, build=True):
    root = (source if source is not None else packages).resolve(strict=True)
    paths = {}
    package_dirs = dict(HFSPLUS_PACKAGES)
    if build:
        package_dirs[HFSPLUS_BUILDER] = HFSPLUS_BUILDER_SOURCE
    if reader:
        package_dirs[HFSPLUS_READER] = HFSPLUS_READER_SOURCE
    for name, source_dir in package_dirs.items():
        path = root / (source_dir if source is not None else f"{name}-{HFSPLUS_VERSION}")
        path = path.resolve(strict=True)
        package = load_toml(path / MANIFEST)["package"]
        if (package["name"], package["version"]) != (name, HFSPLUS_VERSION):
            raise ValueError(f"expected {name} {HFSPLUS_VERSION} in {path}")
        paths[name] = path
    return paths


def compose_cli_manifest(manifest, reader, build):
    sections = tomllib.loads(manifest)
    features = sections["features"]
    additions = {}
    dependencies_to_add = []
    if build:
        for name, forwarding in FEATURE_FORWARDING.items():
            features[name] += forwarding
        additions.update(FEATURE_ADDITIONS)
        dependencies_to_add.extend(("dmgez-builder", ADAPTER))
    if reader:
        additions[READ_FEATURE] = ["inspect", f"dep:{READ_ADAPTER}", f"{READ_ADAPTER}/browse"]
        additions[VERIFY_FEATURE] = ["inspect", f"dep:{READ_ADAPTER}", f"{READ_ADAPTER}/verify"]
        additions[EXTRACT_FEATURE] = [READ_FEATURE, "dep:dmgez-operations", "dmgez-operations/extract"]
        dependencies_to_add.append(READ_ADAPTER)
    if features.keys() & additions.keys():
        raise ValueError("CLI manifest already has HFS+ composition features")
    features.update(additions)

    feature_section = "[features]\n"
    dependency_section = "\n[dependencies]\n"
    before_features, _, rest = manifest.partition(feature_section)
    _, boundary, dependencies = rest.partition(dependency_section)
    if not boundary:
        raise ValueError("could not find the CLI features and dependencies sections")
    lines = "".join(f"{name} = {json.dumps(values)}\n" for name, values in features.items())
    added_dependencies = "".join(
        f'{name} = {{ path = "../../crates/services/{name}", '
        "optional = true, default-features = false }\n"
        for name in dependencies_to_add
    )
    return (
        before_features + feature_section + lines + dependency_section
        + added_dependencies + dependencies
    )


def prepare_workspace(directory, hfsplus):
    for name in SOURCE_DIRS:
        shutil.copytree(
            ROOT / name,
            directory / name,
            ignore=shutil.ignore_patterns("target", ".git", "__pycache__"),
            symlinks=True,
        )
    for name in (LOCK, TOOLCHAIN, "rustfmt.toml", "LICENSE"):
        shutil.copyfile(ROOT / name, directory / name)

    manifest = (ROOT / MANIFEST).read_text(encoding="utf-8")
    if "patch" in tomllib.loads(manifest):
        raise ValueError("workspace manifest already has registry patches")
    patches = "".join(
        f"{name} = {{ path = {json.dumps(str(path), ensure_ascii=False)} }}\n"
        for name, path in hfsplus.items()
    )
    (directory / MANIFEST).write_text(
        manifest + "\n[patch.crates-io]\n" + patches, encoding="utf-8",
    )

    # Replace the copied entry or symlink without writing through to the checkout.
    cli_manifest = directory / CLI / MANIFEST
    if not cli_manifest.parent.resolve().is_relative_to(directory.resolve()):
        raise ValueError("copied CLI directory points outside the temporary workspace")
    text = compose_cli_manifest(
        cli_manifest.read_text(encoding="utf-8"),
        reader=HFSPLUS_READER in hfsplus,
        build=HFSPLUS_BUILDER in hfsplus,
    )
    cli_manifest.unlink()
    cli_manifest.write_text(text, encoding="utf-8")


def registry_packages(lock):
    return {
        (package["name"], package["version"], package["source"], package.get("checksum"))
        for package in lock["package"]
        if "source" in package
    }


def resolve_lock(cargo, directory, hfsplus):
    path = directory / LOCK
    before = load_toml(path)
    subprocess.run([*cargo, "update", "--workspace", "--offline"], cwd=directory, check=True)
    after = load_toml(path)
    if registry_packages(before) != registry_packages(after):
        raise ValueError("HFS+ composition changed locked registry dependencies")
    local = {
        (package["name"], package["version"])
        for package in after["package"]
        if "source" not in package
    }
    if any((name, HFSPLUS_VERSION) not in local for name in hfsplus):
        raise ValueError("temporary lockfile did not resolve all local HFS+ packages")


def run_cli(cargo, directory, options, hfsplus):
    command = [
        *cargo, "test" if options.test else "build", "--frozen", "--package", CLI_PACKAGE,
        "--target-dir", str(options.target_dir.resolve()),
    ]
    if options.release:
        command.append("--release")
    if options.no_default_features:
        command.append("--no-default-features")
    for features in options.features:
        command.extend(("--features", features))

    if options.test:
        environment = os.environ.copy()
        if HFSPLUS_READER in hfsplus:
            fixtures = hfsplus[HFSPLUS_READER] / "tests" / "fixtures"
            environment.setdefault(HFSPLUS_FIXTURES_ENV, str(fixtures))
            if has_cli_feature(options, VERIFY_FEATURE):
                command.extend(("--test", VERIFY_TEST))
            if any(has_cli_feature(options, name) for name in READ_PROFILES):
                command.extend(("--test", READ_TEST))
            if has_cli_feature(options, EXTRACT_FEATURE):
                command.extend(("--test", EXTRACT_TEST))
        if HFSPLUS_BUILDER in hfsplus:
            command.extend(("--test", CLI_TEST))
        subprocess.run(command, cwd=directory, env=environment, check=True)
        return

    command.extend(("--bin", CLI_BINARY, "--message-format=json-render-diagnostics"))
    artifact = None
    with subprocess.Popen(command, cwd=directory, stdout=subprocess.PIPE, encoding="utf-8") as process:
        for line in process.stdout:
            message = json.loads(line)
            if message.get("reason") == "compiler-artifact" and message.get("executable"):
                target = message["target"]
                if target["name"] == CLI_BINARY and target["kind"] == ["bin"]:
                    artifact = message["executable"]
        status = process.wait()
    if status:
        raise subprocess.CalledProcessError(status, command)
    if artifact is None:
        raise ValueError("Cargo did not report a dmgez binary")
    print(artifact)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    inputs = parser.add_mutually_exclusive_group(required=True)
    inputs.add_argument("--hfsplus", type=Path, help="HFS+ source checkout")
    inputs.add_argument("--hfsplus-packages", type=Path, help="unpacked HFS+ crate archives")
    parser.add_argument(
        "--target-dir", type=Path, default=ROOT / "target/hfsplus-cli",
        help="Cargo artifact directory (default: target/hfsplus-cli)",
    )
    parser.add_argument("--release", action="store_true", help="build the optimized binary")
    parser.add_argument("--test", action="store_true", help="run the selected composed CLI tests")
    parser.add_argument("--no-default-features", action="store_true", help="disable default CLI features")
    parser.add_argument("--features", action="append", default=[], help="enable Cargo features, repeatable")
    options = parser.parse_args()
    if sys.platform != "darwin" and not sys.platform.startswith("linux"):
        parser.error("HFS+ CLI composition requires a macOS or Linux host")

    try:
        reader = any(has_cli_feature(options, name) for name in (*READ_PROFILES, VERIFY_FEATURE))
        build = not reader or not options.no_default_features or any(
            has_cli_feature(options, name) for name in ("build", *FEATURE_ADDITIONS)
        )
        hfsplus = resolve_hfsplus(
            options.hfsplus, options.hfsplus_packages, reader=reader, build=build,
        )
        cargo = ["rustup", "run", load_toml(ROOT / TOOLCHAIN)["toolchain"]["channel"], "cargo"]
        with TemporaryDirectory(prefix="dmgez-hfsplus-cli.") as temporary:
            directory = Path(temporary)
            prepare_workspace(directory, hfsplus)
            resolve_lock(cargo, directory, hfsplus)
            run_cli(cargo, directory, options, hfsplus)
    except subprocess.CalledProcessError as error:
        return error.returncode
    except (OSError, ValueError, KeyError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
