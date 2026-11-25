#!/usr/bin/env python3
"""Build dmgez with locally supplied, versioned Pomologie filesystem packages."""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory
import tomllib


ROOT = Path(__file__).resolve().parents[1]
CLI = Path("apps/dmgez-cli")
CLI_PACKAGE = "dmgez-cli"
CLI_BINARY = "dmgez"
DESKTOP = Path("apps/dmgez")
DESKTOP_PACKAGE = "dmgez-gui"
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
ACTUAL_PREVIEW_FEATURE = "actual-preview"
PREVIEW_TEST = "preview_cli"
HFS_ANALYSIS_FEATURE = "hfsplus-analysis"
VERIFY_FEATURE = "hfsplus-verify"
EXTRACT_FEATURE = "hfsplus-extract"
DECMPFS_FEATURE = "hfsplus-decmpfs"
INSTALL_FEATURE = "hfsplus-install"
MOUNT_WATCH_FEATURE = "mount-watch"
MOUNT_TEST = "mount_cli"
PARTITION_WRITE_FEATURE = "partition-write"
INSTALLER_TEST_FEATURE = "installer-test"
DIFF_FEATURE = "diff"
DIFF_HASH_FEATURE = "diff-hash"
FINDER_DIFF_FEATURE = "finder-diff"
READ_PROFILES = (READ_FEATURE, ACTUAL_PREVIEW_FEATURE, HFS_ANALYSIS_FEATURE, EXTRACT_FEATURE, DECMPFS_FEATURE, INSTALL_FEATURE)
APFS_READER = "apfs-reader"
APFS_PACKAGES = {
    "apfs-core": "crates/foundation/apfs-core",
    "apfs-format": "crates/formats/apfs-format",
    "apfs-container": "crates/container/apfs-container",
    APFS_READER: "crates/filesystem/apfs-reader",
}
APFS_ADAPTER = "dmgez-apfs-read"
COMPOSED_CRATE_GROUPS = {
    "dmgez-builder": "build",
    ADAPTER: "adapters",
    READ_ADAPTER: "adapters",
    APFS_ADAPTER: "adapters",
}
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
SOURCE_DIRS = ("apps", "crates", "tests", "third_party")
RUNTIME = Path("crates/runtime/dmgez-runtime")
PROJECT_FEATURES = {
    "project-build": ["dep:dmgez-builder", f"dep:{ADAPTER}", "dep:dmgez-config", "dmgez-runtime/project-build"],
    PROJECT_RELEASE_FEATURE: ["project-build", "inspect", "dep:dmgez-signing", "dep:sha2", "dep:tempfile"],
    PROJECT_EDIT_FEATURE: [
        "project-build", "dep:dmgez-core", "dep:dmgez-operations", "dmgez-operations/edit",
        "dep:dmgez-udif", "dmgez-udif/read", "dep:serde", "dep:serde_json", "dep:sha2",
    ],
    "compression-cache": ["project-build", "dmgez-builder/compression-cache", f"{ADAPTER}/compression-cache"],
    PARTITION_WRITE_FEATURE: ["project-build", f"{ADAPTER}/partition-write"],
}
FINDER_FEATURES = {
    "finder-write": ["dmgez-builder?/finder-write", f"{ADAPTER}?/finder-write", "dmgez-runtime?/finder-write"],
    "volume-icon": ["finder-write", f"{ADAPTER}?/volume-icon"],
    "badge-icons": ["volume-icon", f"{ADAPTER}?/badge-icons"],
}
FEATURE_FORWARDING = {
    "default": [PROJECT_RELEASE_FEATURE],
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


def render_feature(name, values):
    inline = f"{name} = {json.dumps(values)}"
    if len(inline) <= 100:
        return inline
    entries = "".join(f"\t{json.dumps(value)},\n" for value in values)
    return f"{name} = [\n{entries}]"


def compose_cli_manifest(manifest, reader, build, apfs):
    features = tomllib.loads(manifest)["features"]
    forwarding = FEATURE_FORWARDING.copy() if build else {}
    project_features = PROJECT_FEATURES.copy() if build else {}
    hfsplus_features = {}
    apfs_features = {}
    dependencies_to_add = []
    if build:
        dependencies_to_add.extend(("dmgez-builder", ADAPTER))
    if reader:
        hfsplus_features = {
            READ_FEATURE: ["inspect", f"dep:{READ_ADAPTER}", f"{READ_ADAPTER}/browse"],
            ACTUAL_PREVIEW_FEATURE: ["preview", READ_FEATURE, f"{READ_ADAPTER}/preview"],
            HFS_ANALYSIS_FEATURE: [READ_FEATURE, f"{READ_ADAPTER}/analysis"],
            VERIFY_FEATURE: ["inspect", f"dep:{READ_ADAPTER}", f"{READ_ADAPTER}/verify"],
            EXTRACT_FEATURE: [READ_FEATURE, "dep:dmgez-operations", "dmgez-operations/extract"],
            DECMPFS_FEATURE: [READ_FEATURE, f"{READ_ADAPTER}/decmpfs-zlib"],
            INSTALL_FEATURE: [
                READ_FEATURE, "dep:dmgez-config", "dep:dmgez-operations", "dmgez-operations/install",
            ],
            MOUNT_WATCH_FEATURE: [INSTALL_FEATURE, "mount-system", "dmgez-mount/watch"],
        }
        forwarding[INSTALLER_TEST_FEATURE] = [READ_FEATURE]
        if build:
            project_features[PROJECT_RELEASE_FEATURE] = [
                *project_features[PROJECT_RELEASE_FEATURE], INSTALL_FEATURE,
            ]
            project_features[IMAGE_EDIT_FEATURE] = [
                "project-build", f"{ADAPTER}/image-edit", f"dep:{READ_ADAPTER}", f"{READ_ADAPTER}/browse",
                f"{READ_ADAPTER}/verify",
                "dep:dmgez-core", "dep:dmgez-operations", "dmgez-operations/image-edit", "dep:dmgez-udif",
                "dmgez-udif/read", "dmgez-udif/write", "dep:dmgez-vfs", "dep:serde", "dep:serde_json", "dep:sha2",
            ]
        dependencies_to_add.append(READ_ADAPTER)
    if apfs:
        apfs_features = {
            APFS_FEATURE: ["inspect", f"dep:{APFS_ADAPTER}"],
            APFS_EXTRACT_FEATURE: [APFS_FEATURE, "dep:dmgez-operations", "dmgez-operations/extract"],
            APFS_INSTALL_FEATURE: [
                APFS_FEATURE, "dep:dmgez-config", "dep:dmgez-operations", "dmgez-operations/install",
            ],
        }
        forwarding.setdefault(INSTALLER_TEST_FEATURE, []).append(APFS_FEATURE)
        dependencies_to_add.append(APFS_ADAPTER)

    feature_section = "[features]\n"
    dependency_section = "\n[dependencies]\n"
    before_features, _, rest = manifest.partition(feature_section)
    feature_text, boundary, dependencies = rest.partition(dependency_section)
    if not boundary:
        raise ValueError("could not find the CLI features and dependencies sections")
    for name, extra in forwarding.items():
        assignment = rf"(?m)^{re.escape(name)} = \[(?:[^\n]*?\]|[\s\S]*?^\])"
        replacement = render_feature(name, [*features[name], *extra])
        feature_text, matches = re.subn(assignment, lambda _: replacement, feature_text)
        if matches != 1:
            raise ValueError(f"could not find a single CLI feature assignment for {name}")

    feature_groups = (
        ("HFS+ project build, edit and release", project_features),
        ("HFS+ Finder metadata", FINDER_FEATURES if build else {}),
        ("HFS+ volume access", hfsplus_features),
        ("APFS volume access", apfs_features),
    )
    declared_features = set(features)
    for title, group in feature_groups:
        if declared_features.intersection(group):
            raise ValueError("CLI manifest already has filesystem composition features")
        if group:
            entries = "\n".join(render_feature(name, values) for name, values in group.items())
            feature_text += f"\n# {title}\n{entries}\n"
            declared_features.update(group)

    if dependencies_to_add:
        normal, dev_boundary, development = dependencies.partition("\n[dev-dependencies]\n")
        if not dev_boundary:
            raise ValueError("could not find the CLI dev-dependencies section")
        normal += "\n# Composed filesystem dependencies\n" + "".join(
            f'{name} = {{ path = "../../crates/{COMPOSED_CRATE_GROUPS[name]}/{name}", '
            "optional = true, default-features = false }\n"
            for name in sorted(dependencies_to_add)
        )
        if build:
            normal += "\n# Hashes and checksums\nsha2 = { workspace = true, optional = true }\n"
            normal += "\n# Filesystem access and staging\ntempfile = { workspace = true, optional = true }\n"
        if reader and build:
            development = "# Workspace presentation\ndmgez-finder.workspace = true\n\n" + development
        dependencies = normal + dev_boundary + development
    return before_features + feature_section + feature_text + dependency_section + dependencies


def compose_desktop_manifest(manifest):
    parsed = tomllib.loads(manifest)
    if ({READ_FEATURE, ACTUAL_PREVIEW_FEATURE}.intersection(parsed.get("features", {}))
            or READ_ADAPTER in parsed["dependencies"] or "dmgez-vfs" in parsed["dependencies"]):
        raise ValueError("desktop manifest already has filesystem composition settings")
    feature_section = "\n[features]\n"
    dependency_section = "\n[dependencies]\n"
    development_section = "\n[dev-dependencies]\n"
    if dependency_section not in manifest or development_section not in manifest:
        raise ValueError("could not find the desktop dependencies sections")
    read = render_feature(READ_FEATURE, [f"dep:{READ_ADAPTER}", "dep:dmgez-vfs"])
    preview = render_feature(ACTUAL_PREVIEW_FEATURE, [READ_FEATURE, f"{READ_ADAPTER}/preview"])
    group = f"# HFS+ volume access\n{read}\n{preview}\n"
    if feature_section in manifest:
        manifest = manifest.replace(feature_section, feature_section + group, 1)
    else:
        manifest = manifest.replace(dependency_section, feature_section + group + dependency_section, 1)
    return manifest.replace(
        development_section,
        "\n# Composed filesystem dependencies\n"
        f'{READ_ADAPTER} = {{ path = "../../crates/{COMPOSED_CRATE_GROUPS[READ_ADAPTER]}/{READ_ADAPTER}", '
        'optional = true, default-features = false, features = ["browse"] }\n'
        "dmgez-vfs = { workspace = true, optional = true }\n"
        + development_section,
        1,
    )


def compose_runtime_manifest(manifest):
    dependency_section = "\n[dependencies]\n"
    dependencies = [
        'dmgez-hfsplus = { path = "../../adapters/dmgez-hfsplus", optional = true, default-features = false }',
    ]
    features = [
        render_feature("project-build", [
            "project-input", "dep:dmgez-builder", "dep:dmgez-hfsplus", "dep:dmgez-udif", "dmgez-udif/write",
        ]),
        render_feature("finder-write", ["dmgez-builder?/finder-write"]),
    ]
    before, boundary, after = manifest.partition(dependency_section)
    if not boundary:
        raise ValueError("could not find the runtime dependencies section")
    return before + "\n# Composed project workflows\n" + "\n".join(features) + "\n" + (
        boundary + "# Composed filesystem dependencies\n" + "\n".join(dependencies) + "\n\n" + after
    )


def replace_manifest(directory, relative, transform):
    path = directory / relative / MANIFEST
    if not path.parent.resolve().is_relative_to(directory.resolve()):
        raise ValueError("copied manifest directory points outside the temporary workspace")
    updated = transform(path.read_text(encoding="utf-8"))
    path.unlink()
    path.write_text(updated, encoding="utf-8")


def prepare_workspace(directory, packages, *, desktop=False):
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
    build = HFSPLUS_BUILDER in packages
    if build:
        replace_manifest(directory, RUNTIME, compose_runtime_manifest)
    if desktop:
        replace_manifest(directory, DESKTOP, compose_desktop_manifest)
    else:
        replace_manifest(directory, CLI, lambda manifest: compose_cli_manifest(
            manifest,
            reader=HFSPLUS_READER in packages,
            build=build,
            apfs=APFS_READER in packages,
        ))


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


def cargo_command(cargo, options, package):
    command = [
        *cargo, "test" if options.test else "build", "--frozen", "--package", package,
        "--target-dir", str(options.target_dir.resolve()),
    ]
    if options.release:
        command.append("--release")
    if options.no_default_features:
        command.append("--no-default-features")
    for features in options.features:
        command.extend(("--features", features))
    return command


def run_cli(cargo, directory, options, packages):
    command = cargo_command(cargo, options, CLI_PACKAGE)

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
            if has_cli_feature(options, ACTUAL_PREVIEW_FEATURE):
                command.extend(("--test", PREVIEW_TEST))
        if any(has_cli_feature(options, name) for name in (INSTALL_FEATURE, APFS_INSTALL_FEATURE)):
            command.extend(("--test", INSTALL_TEST))
        if has_cli_feature(options, MOUNT_WATCH_FEATURE):
            command.extend(("--test", MOUNT_TEST))
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

    run_binary(command, directory, CLI_BINARY)


def run_desktop(cargo, directory, options, packages):
    command = cargo_command(cargo, options, DESKTOP_PACKAGE)
    command.extend(("--features", READ_FEATURE))
    if options.test:
        environment = os.environ.copy()
        fixtures = packages[HFSPLUS_READER] / "tests" / "fixtures"
        environment.setdefault(HFSPLUS_FIXTURES_ENV, str(fixtures))
        subprocess.run([*command, "--bin", DESKTOP_PACKAGE], cwd=directory, env=environment, check=True)
        return
    run_binary(command, directory, DESKTOP_PACKAGE)


def run_binary(command, directory, binary):
    command.extend(("--bin", binary, "--message-format=json-render-diagnostics"))
    artifact = None
    with subprocess.Popen(command, cwd=directory, stdout=subprocess.PIPE, encoding="utf-8") as process:
        for line in process.stdout:
            message = json.loads(line)
            if message.get("reason") == "compiler-artifact" and message.get("executable"):
                target = message["target"]
                if target["name"] == binary and target["kind"] == ["bin"]:
                    artifact = message["executable"]
        status = process.wait()
    if status:
        raise subprocess.CalledProcessError(status, command)
    if artifact is None:
        raise ValueError(f"Cargo did not report a {binary} binary")
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
        help="Cargo artifact directory (defaults to a target directory for the selected profile)",
    )
    parser.add_argument("--release", action="store_true", help="build the optimized binary")
    parser.add_argument("--desktop", action="store_true", help="compose the desktop app with the HFS+ reader")
    parser.add_argument("--test", action="store_true", help="run the selected composed app tests")
    parser.add_argument(
        "--fetch", action="store_true",
        help="download dependencies from the checked-in Cargo lockfile before offline composition",
    )
    parser.add_argument("--no-default-features", action="store_true", help="disable default CLI features")
    parser.add_argument("--features", action="append", default=[], help="enable Cargo features, repeatable")
    options = parser.parse_args()
    use_hfsplus = options.hfsplus is not None or options.hfsplus_packages is not None
    use_apfs = options.apfs is not None or options.apfs_packages is not None
    if not (use_hfsplus or use_apfs):
        parser.error("supply an HFS+ or APFS package source")
    if options.desktop and (not use_hfsplus or use_apfs):
        parser.error("desktop composition requires only an HFS+ package source")
    for feature in APFS_PROFILES:
        if has_cli_feature(options, feature) and not use_apfs:
            parser.error(f"{feature} requires --apfs or --apfs-packages")
    for feature in (ACTUAL_PREVIEW_FEATURE, HFS_ANALYSIS_FEATURE, INSTALL_FEATURE, MOUNT_WATCH_FEATURE, PROJECT_EDIT_FEATURE, IMAGE_EDIT_FEATURE, PARTITION_WRITE_FEATURE):
        if has_cli_feature(options, feature) and not use_hfsplus:
            parser.error(f"{feature} requires --hfsplus or --hfsplus-packages")
    if use_apfs and not any(has_cli_feature(options, feature) for feature in APFS_PROFILES):
        options.features.append(APFS_FEATURE)
    if options.target_dir is None:
        profile = "hfsplus-desktop" if options.desktop else "apfs-cli" if use_apfs else "hfsplus-cli"
        options.target_dir = ROOT / "target" / profile
    explicit_read = any(has_cli_feature(options, name) for name in (*READ_PROFILES, VERIFY_FEATURE, MOUNT_WATCH_FEATURE)) or (
        use_hfsplus and (
            has_cli_feature(options, INSTALLER_TEST_FEATURE) or
            sys.platform == "darwin" and has_cli_feature(options, PROJECT_RELEASE_FEATURE)
        )
    )
    if sys.platform == "win32":
        if not options.desktop and (use_apfs or explicit_read):
            parser.error("Windows composition supports HFS+ project builds and image editing only")
    elif sys.platform != "darwin" and not sys.platform.startswith("linux"):
        parser.error("filesystem CLI composition requires a macOS, Linux, or Windows host")

    try:
        image_edit = has_cli_feature(options, IMAGE_EDIT_FEATURE)
        reader = options.desktop or explicit_read or image_edit
        build = not options.desktop and (image_edit or not reader or not options.no_default_features or any(
            has_cli_feature(options, name) for name in ("build", *PROJECT_FEATURES, *FINDER_FEATURES)
        ))
        packages = {}
        if use_hfsplus:
            packages.update(resolve_hfsplus(
                options.hfsplus, options.hfsplus_packages, reader=reader, build=build,
            ))
        if use_apfs:
            packages.update(resolve_packages(options.apfs, options.apfs_packages, APFS_PACKAGES))
        cargo = ["rustup", "run", load_toml(ROOT / TOOLCHAIN)["toolchain"]["channel"], "cargo"]
        if options.fetch:
            subprocess.run([*cargo, "fetch", "--locked"], cwd=ROOT, check=True)
        with TemporaryDirectory(prefix="dmgez-filesystem-cli.") as temporary:
            directory = Path(temporary)
            if options.desktop:
                prepare_workspace(directory, packages, desktop=True)
            else:
                prepare_workspace(directory, packages)
            resolve_lock(cargo, directory, packages)
            (run_desktop if options.desktop else run_cli)(cargo, directory, options, packages)
    except subprocess.CalledProcessError as error:
        return error.returncode
    except (OSError, ValueError, KeyError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
