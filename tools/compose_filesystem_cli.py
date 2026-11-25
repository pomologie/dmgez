#!/usr/bin/env python3
"""Build dmgez with locally supplied, versioned Pomologie filesystem packages."""

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
WINDOWS_CLI_TEST = "windows_project_build_cli"
VERIFY_TEST = "hfs_verify_cli"
READ_TEST = "volume_cli"
APFS_VERIFY_TEST = "apfs_verify_cli"
APFS_EXTRACT_TEST = "apfs_extract_cli"
EXTRACT_TEST = "extract_cli"
INSTALL_TEST = "install_cli"
INSTALLER_TEST = "installer_test_cli"
DECMPFS_TEST = "hfs_decmpfs_cli"
SEMANTIC_DIFF_TEST = "semantic_diff_cli"
TOOLCHAIN = "rust-toolchain.toml"
MANIFEST = "Cargo.toml"
LOCK = "Cargo.lock"
ENGINE_VERSION = "0.1.0"
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
DECMPFS_FEATURE = "hfsplus-decmpfs"
INSTALL_FEATURE = "hfsplus-install"
INSTALLER_TEST_FEATURE = "installer-test"
DIFF_FEATURE = "diff"
DIFF_HASH_FEATURE = "diff-hash"
FINDER_DIFF_FEATURE = "finder-diff"
READ_PROFILES = (READ_FEATURE, EXTRACT_FEATURE, DECMPFS_FEATURE, INSTALL_FEATURE)
APFS_READER = "apfs-reader"
APFS_PACKAGES = {
    "apfs-core": "crates/foundation/apfs-core",
    "apfs-format": "crates/formats/apfs-format",
    "apfs-container": "crates/container/apfs-container",
    APFS_READER: "crates/filesystem/apfs-reader",
}
APFS_ADAPTER = "dmgez-apfs-read"
APFS_FEATURE = "apfs-read"
APFS_EXTRACT_FEATURE = "apfs-extract"
APFS_INSTALL_FEATURE = "apfs-install"
APFS_PROFILES = (APFS_FEATURE, APFS_EXTRACT_FEATURE, APFS_INSTALL_FEATURE)
APFS_FIXTURES_ENV = "DMGEZ_APFS_FIXTURES"
PROJECT_RELEASE_FEATURE = "project-release"
PROJECT_EDIT_FEATURE = "project-edit"
PROJECT_EDIT_TEST = "project_edit_cli"
IMAGE_EDIT_FEATURE = "image-edit"
IMAGE_EDIT_TEST = "image_edit_cli"
SOURCE_DIRS = ("apps", "crates", "tests")
FEATURE_ADDITIONS = {
    "project-build": ["dep:dmgez-builder", f"dep:{ADAPTER}", "dep:dmgez-config"],
    PROJECT_RELEASE_FEATURE: ["project-build", "inspect", "dep:dmgez-signing", "dep:sha2", "dep:tempfile"],
    PROJECT_EDIT_FEATURE: [
        "project-build", "dep:dmgez-core", "dep:dmgez-operations", "dmgez-operations/edit",
        "dep:dmgez-udif", "dmgez-udif/read", "dep:serde", "dep:serde_json", "dep:sha2",
    ],
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


def resolve_packages(source, packages, package_dirs):
    root = (source if source is not None else packages).resolve(strict=True)
    paths = {}
    for name, source_dir in package_dirs.items():
        path = root / (source_dir if source is not None else f"{name}-{ENGINE_VERSION}")
        path = path.resolve(strict=True)
        package = load_toml(path / MANIFEST)["package"]
        if (package["name"], package["version"]) != (name, ENGINE_VERSION):
            raise ValueError(f"expected {name} {ENGINE_VERSION} in {path}")
        paths[name] = path
    return paths


def resolve_hfsplus(source, packages, reader, build):
    package_dirs = dict(HFSPLUS_PACKAGES)
    if build:
        package_dirs[HFSPLUS_BUILDER] = HFSPLUS_BUILDER_SOURCE
    if reader:
        package_dirs[HFSPLUS_READER] = HFSPLUS_READER_SOURCE
    return resolve_packages(source, packages, package_dirs)


def compose_cli_manifest(manifest, reader, build, apfs):
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
        features[INSTALLER_TEST_FEATURE].append(READ_FEATURE)
        additions[VERIFY_FEATURE] = ["inspect", f"dep:{READ_ADAPTER}", f"{READ_ADAPTER}/verify"]
        additions[EXTRACT_FEATURE] = [READ_FEATURE, "dep:dmgez-operations", "dmgez-operations/extract"]
        additions[DECMPFS_FEATURE] = [READ_FEATURE, f"{READ_ADAPTER}/decmpfs-zlib"]
        additions[INSTALL_FEATURE] = [
            READ_FEATURE, "dep:dmgez-config", "dep:dmgez-operations", "dmgez-operations/install",
        ]
        if build:
            additions[PROJECT_RELEASE_FEATURE] = [*additions[PROJECT_RELEASE_FEATURE], INSTALL_FEATURE]
            additions[IMAGE_EDIT_FEATURE] = [
                "project-build", f"{ADAPTER}/image-edit", f"dep:{READ_ADAPTER}", f"{READ_ADAPTER}/browse",
                f"{READ_ADAPTER}/verify",
                "dep:dmgez-core", "dep:dmgez-operations", "dmgez-operations/image-edit", "dep:dmgez-udif",
                "dmgez-udif/read", "dmgez-udif/write", "dep:dmgez-vfs", "dep:serde", "dep:serde_json", "dep:sha2",
            ]
        dependencies_to_add.append(READ_ADAPTER)
    if apfs:
        additions[APFS_FEATURE] = ["inspect", f"dep:{APFS_ADAPTER}"]
        features[INSTALLER_TEST_FEATURE].append(APFS_FEATURE)
        additions[APFS_EXTRACT_FEATURE] = [
            APFS_FEATURE, "dep:dmgez-operations", "dmgez-operations/extract",
        ]
        additions[APFS_INSTALL_FEATURE] = [
            APFS_FEATURE, "dep:dmgez-config", "dep:dmgez-operations", "dmgez-operations/install",
        ]
        dependencies_to_add.append(APFS_ADAPTER)
    if features.keys() & additions.keys():
        raise ValueError("CLI manifest already has filesystem composition features")
    features.update(additions)

    feature_section = "[features]\n"
    dependency_section = "\n[dependencies]\n"
    before_features, _, rest = manifest.partition(feature_section)
    _, boundary, dependencies = rest.partition(dependency_section)
    if not boundary:
        raise ValueError("could not find the CLI features and dependencies sections")
    if reader and build:
        normal, dev_boundary, development = dependencies.partition("\n[dev-dependencies]\n")
        if not dev_boundary:
            raise ValueError("could not find the CLI dev-dependencies section")
        dependencies = normal + dev_boundary + "dmgez-finder.workspace = true\n" + development
    lines = "".join(f"{name} = {json.dumps(values)}\n" for name, values in features.items())
    added_dependencies = "".join(
        f'{name} = {{ path = "../../crates/services/{name}", '
        "optional = true, default-features = false }\n"
        for name in dependencies_to_add
    )
    if build:
        added_dependencies += "sha2 = { workspace = true, optional = true }\n"
        added_dependencies += "tempfile = { workspace = true, optional = true }\n"
    return (
        before_features + feature_section + lines + dependency_section
        + added_dependencies + dependencies
    )


def prepare_workspace(directory, packages):
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
        for name, path in packages.items()
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
        reader=HFSPLUS_READER in packages,
        build=HFSPLUS_BUILDER in packages,
        apfs=APFS_READER in packages,
    )
    cli_manifest.unlink()
    cli_manifest.write_text(text, encoding="utf-8")


def registry_packages(lock):
    return {
        (package["name"], package["version"], package["source"], package.get("checksum"))
        for package in lock["package"]
        if "source" in package
    }


def resolve_lock(cargo, directory, packages):
    path = directory / LOCK
    before = load_toml(path)
    subprocess.run([*cargo, "update", "--workspace", "--offline"], cwd=directory, check=True)
    after = load_toml(path)
    if registry_packages(before) != registry_packages(after):
        raise ValueError("filesystem composition changed locked registry dependencies")
    local = {
        (package["name"], package["version"])
        for package in after["package"]
        if "source" not in package
    }
    if any((name, ENGINE_VERSION) not in local for name in packages):
        raise ValueError("temporary lockfile did not resolve all local filesystem packages")


def run_cli(cargo, directory, options, packages):
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
        if APFS_READER in packages:
            fixtures = packages[APFS_READER] / "tests" / "fixtures"
            environment.setdefault(APFS_FIXTURES_ENV, str(fixtures))
            command.extend(("--test", APFS_VERIFY_TEST))
            if has_cli_feature(options, APFS_EXTRACT_FEATURE):
                command.extend(("--test", APFS_EXTRACT_TEST))
        browse = APFS_READER in packages or any(
            has_cli_feature(options, name) for name in READ_PROFILES
        )
        if browse:
            command.extend(("--test", READ_TEST))
            if any(has_cli_feature(options, name) for name in (DIFF_FEATURE, DIFF_HASH_FEATURE, FINDER_DIFF_FEATURE)):
                command.extend(("--test", SEMANTIC_DIFF_TEST))
        if HFSPLUS_READER in packages:
            fixtures = packages[HFSPLUS_READER] / "tests" / "fixtures"
            environment.setdefault(HFSPLUS_FIXTURES_ENV, str(fixtures))
            if has_cli_feature(options, VERIFY_FEATURE):
                command.extend(("--test", VERIFY_TEST))
            if any(has_cli_feature(options, name) for name in READ_PROFILES):
                command.extend(("--test", DECMPFS_TEST))
            if has_cli_feature(options, EXTRACT_FEATURE):
                command.extend(("--test", EXTRACT_TEST))
        if any(has_cli_feature(options, name) for name in (INSTALL_FEATURE, APFS_INSTALL_FEATURE)):
            command.extend(("--test", INSTALL_TEST))
        if has_cli_feature(options, INSTALLER_TEST_FEATURE):
            command.extend(("--test", INSTALLER_TEST))
        if HFSPLUS_BUILDER in packages:
            command.extend(("--test", CLI_TEST))
            if has_cli_feature(options, PROJECT_EDIT_FEATURE):
                command.extend(("--test", PROJECT_EDIT_TEST))
            if has_cli_feature(options, IMAGE_EDIT_FEATURE):
                command.extend(("--test", IMAGE_EDIT_TEST))
            if has_cli_feature(options, PROJECT_RELEASE_FEATURE):
                command.extend(("--bin", CLI_BINARY))
            if sys.platform == "win32":
                command.extend(("--test", WINDOWS_CLI_TEST))
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
    inputs = parser.add_mutually_exclusive_group()
    inputs.add_argument("--hfsplus", type=Path, help="HFS+ source checkout")
    inputs.add_argument("--hfsplus-packages", type=Path, help="unpacked HFS+ crate archives")
    apfs = parser.add_mutually_exclusive_group()
    apfs.add_argument("--apfs", type=Path, help="APFS source checkout")
    apfs.add_argument("--apfs-packages", type=Path, help="unpacked APFS crate archives")
    parser.add_argument(
        "--target-dir", type=Path,
        help="Cargo artifact directory (default: target/apfs-cli or target/hfsplus-cli)",
    )
    parser.add_argument("--release", action="store_true", help="build the optimized binary")
    parser.add_argument("--test", action="store_true", help="run the selected composed CLI tests")
    parser.add_argument("--no-default-features", action="store_true", help="disable default CLI features")
    parser.add_argument("--features", action="append", default=[], help="enable Cargo features, repeatable")
    options = parser.parse_args()
    use_hfsplus = options.hfsplus is not None or options.hfsplus_packages is not None
    use_apfs = options.apfs is not None or options.apfs_packages is not None
    if not (use_hfsplus or use_apfs):
        parser.error("supply an HFS+ or APFS package source")
    for feature in APFS_PROFILES:
        if has_cli_feature(options, feature) and not use_apfs:
            parser.error(f"{feature} requires --apfs or --apfs-packages")
    for feature in (INSTALL_FEATURE, PROJECT_EDIT_FEATURE, IMAGE_EDIT_FEATURE):
        if has_cli_feature(options, feature) and not use_hfsplus:
            parser.error(f"{feature} requires --hfsplus or --hfsplus-packages")
    if use_apfs and not any(has_cli_feature(options, feature) for feature in APFS_PROFILES):
        options.features.append(APFS_FEATURE)
    if options.target_dir is None:
        options.target_dir = ROOT / "target" / ("apfs-cli" if use_apfs else "hfsplus-cli")
    explicit_read = any(has_cli_feature(options, name) for name in (*READ_PROFILES, VERIFY_FEATURE)) or (
        use_hfsplus and (
            has_cli_feature(options, INSTALLER_TEST_FEATURE) or
            sys.platform == "darwin" and has_cli_feature(options, PROJECT_RELEASE_FEATURE)
        )
    )
    if sys.platform == "win32":
        if use_apfs or explicit_read:
            parser.error("Windows composition supports HFS+ project builds and image editing only")
    elif sys.platform != "darwin" and not sys.platform.startswith("linux"):
        parser.error("filesystem CLI composition requires a macOS, Linux, or Windows host")

    try:
        image_edit = has_cli_feature(options, IMAGE_EDIT_FEATURE)
        reader = explicit_read or image_edit
        build = image_edit or not reader or not options.no_default_features or any(
            has_cli_feature(options, name) for name in ("build", *FEATURE_ADDITIONS)
        )
        packages = {}
        if use_hfsplus:
            packages.update(resolve_hfsplus(
                options.hfsplus, options.hfsplus_packages, reader=reader, build=build,
            ))
        if use_apfs:
            packages.update(resolve_packages(options.apfs, options.apfs_packages, APFS_PACKAGES))
        cargo = ["rustup", "run", load_toml(ROOT / TOOLCHAIN)["toolchain"]["channel"], "cargo"]
        with TemporaryDirectory(prefix="dmgez-filesystem-cli.") as temporary:
            directory = Path(temporary)
            prepare_workspace(directory, packages)
            resolve_lock(cargo, directory, packages)
            run_cli(cargo, directory, options, packages)
    except subprocess.CalledProcessError as error:
        return error.returncode
    except (OSError, ValueError, KeyError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
