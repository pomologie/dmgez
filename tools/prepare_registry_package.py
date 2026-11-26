"""Prepare the source and lockfile for the standalone dmgez Cargo package."""

import argparse
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

import compose_filesystem_cli as compose
import registry_manifest
import registry_source_tree as source_tree


RELEASE_TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
)
PACKAGE = "dmgez"
COMPOSED_PROFILE = "inspect"
TREE_OPTIONS = ("-e", "normal,build", "--no-dedupe", "--prefix", "depth", "--format", "{p}|{f}")


def extract_snapshot(snapshot, destination, archive):
    with archive.open("wb") as stream:
        # Git archive applies checkout conversions, including the Windows CRLF default.
        subprocess.run([
            "git", "-C", snapshot.repository, "-c", "core.autocrlf=false", "-c", "core.eol=lf",
            "archive", "--format=tar", snapshot.revision,
        ], stdout=stream, check=True)
    with tarfile.open(archive, "r:") as contents:
        contents.extractall(destination, filter="data")


def capture(cargo, directory, output, *arguments):
    with output.open("wb") as stream:
        subprocess.run([*cargo, *arguments], cwd=directory, stdout=stream, check=True)


def verify_registry(cargo, package, scratch, inventory):
    lock = registry_manifest.read_lock(package / compose.LOCK)
    for target, selected in inventory["targets"].items():
        tree = scratch / f"standalone-{target}.tree"
        capture(cargo, package, tree, "tree", "--locked", "--offline", "--target", target, *TREE_OPTIONS)
        packages, _, root = registry_manifest.read_tree(tree)
        if root[0] != PACKAGE:
            raise ValueError(f"unexpected standalone package {root[0]}")
        del packages[root]
        expected = {
            (entry["name"], entry["version"]): (entry["checksum"], set(entry["feature_sets"]))
            for entry in selected["registry"]
        }
        actual = {key: (lock.get(key), features) for key, features in packages.items()}
        if actual != expected:
            changes = sorted(key for key in actual.keys() | expected.keys() if actual.get(key) != expected.get(key))
            raise ValueError(f"standalone dependencies differ on {target}: {changes}")


def prepare(output, xtask, dmgez, hfsplus, fetch):
    if output.exists():
        raise ValueError(f"output already exists: {output}")
    with tempfile.TemporaryDirectory(prefix="dmgez-registry-prepare-") as temporary:
        scratch = Path(temporary)
        dmgez_source = scratch / "dmgez"
        hfsplus_source = scratch / "hfsplus"
        extract_snapshot(dmgez, dmgez_source, scratch / "dmgez.tar")
        extract_snapshot(hfsplus, hfsplus_source, scratch / "hfsplus.tar")
        channel = compose.load_toml(dmgez_source / compose.TOOLCHAIN)["toolchain"]["channel"]
        cargo = ["rustup", "run", channel, "cargo"]
        workspace = scratch / "composed"
        sources = compose.resolve_hfsplus(hfsplus_source, None, reader=False, build=True)
        compose.prepare_workspace(workspace, sources, source_root=dmgez_source)
        compose.resolve_lock(cargo, workspace, sources)
        if fetch:
            subprocess.run([*cargo, "fetch", "--locked"], cwd=workspace, check=True)
        metadata = {}
        trees = {}
        for target in RELEASE_TARGETS:
            metadata[target] = scratch / f"{target}.json"
            trees[target] = scratch / f"{target}.tree"
            capture(
                cargo, workspace, metadata[target], "metadata", "--locked", "--offline",
                "--format-version", "1", "--filter-platform", target,
                "--manifest-path", compose.CLI / compose.MANIFEST, "--features", COMPOSED_PROFILE,
            )
            capture(
                cargo, workspace, trees[target], "tree", "--locked", "--offline",
                "-p", compose.CLI_PACKAGE, "--features", COMPOSED_PROFILE,
                "--target", target, *TREE_OPTIONS,
            )
        lock = workspace / compose.LOCK
        manifest, inventory = registry_manifest.generate(
            metadata, trees, registry_manifest.read_lock(lock), compose.CLI_PACKAGE,
        )
        package = scratch / "standalone"
        package.mkdir()
        (package / compose.MANIFEST).write_text(manifest, encoding="utf-8", newline="\n")
        source_tree.assemble(inventory, package, xtask, dmgez, hfsplus)
        shutil.copyfile(lock, package / compose.LOCK)
        subprocess.run([*cargo, "update", "--workspace", "--offline", "--quiet"], cwd=package, check=True)
        verify_registry(cargo, package, scratch, inventory)
        shutil.copytree(package, output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--xtask", type=Path, required=True, help="built xtask executable")
    parser.add_argument("--dmgez-revision", default="HEAD")
    parser.add_argument("--hfsplus", type=Path, default=source_tree.ROOT.parent / "hfsplus")
    parser.add_argument("--fetch", action="store_true", help="prefetch the locked Cargo dependencies")
    arguments = parser.parse_args()
    try:
        prepare(
            arguments.output.resolve(), arguments.xtask.resolve(strict=True),
            source_tree.Snapshot(source_tree.ROOT, arguments.dmgez_revision),
            source_tree.Snapshot(arguments.hfsplus, source_tree.HFSPLUS_PIN), arguments.fetch,
        )
    except (KeyError, OSError, TypeError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"{parser.prog}: {error}\n")


if __name__ == "__main__":
    main()
