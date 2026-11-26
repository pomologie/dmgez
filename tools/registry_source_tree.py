"""Assemble the private implementation modules of the dmgez registry package."""

import argparse
import hashlib
import io
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile


ROOT = Path(__file__).resolve().parents[1]
HFSPLUS_PIN = "a1b870fd04ef7b6ec7e9c5dbfd3f0cc1bf42c48b"
HFSPLUS_PACKAGES = {
    "hfsplus-builder": Path("crates/filesystem/hfsplus-builder"),
    "hfsplus-format": Path("crates/formats/hfsplus-format"),
    "hfsplus-pomologie-core": Path("crates/foundation/hfsplus-pomologie-core"),
}
IDENTIFIER = re.compile(r"[a-z_][a-z0-9_]*\Z")
SIGNER_PATH = Path("third_party/apple-codesign")
EXAMPLE = Path("examples/offline-cli")
README = Path("docs/features/REGISTRY_PACKAGE.md")


def git(repository, *arguments):
    return subprocess.check_output(["git", "-C", repository, *arguments])


class Snapshot:
    def __init__(self, repository, revision):
        self.repository = repository
        self.revision = git(repository, "rev-parse", "--verify", f"{revision}^{{commit}}").decode().strip()
        self.hash_algorithm = git(repository, "rev-parse", "--show-object-format").decode().strip()
        self.blobs = {}
        for entry in git(repository, "ls-tree", "-rz", self.revision).decode().split("\0"):
            if entry:
                header, filename = entry.split("\t", 1)
                mode, kind, oid = header.split()
                if kind == "blob" and mode in {"100644", "100755"}:
                    self.blobs[Path(filename)] = oid

    def read(self, path):
        if path not in self.blobs:
            raise ValueError(f"unrecorded source {path} at {self.revision}")
        return git(self.repository, "cat-file", "blob", self.blobs[path])

    def check(self, path, data):
        header = f"blob {len(data)}\0".encode()
        identity = hashlib.new(self.hash_algorithm, header + data).hexdigest()
        if self.blobs.get(path) != identity:
            raise ValueError(f"source {path} does not match {self.revision}")


def validated_sources(source_path, snapshot, original_source):
    source = source_path.parent
    expected = {
        path.relative_to(original_source)
        for path in snapshot.blobs
        if path.is_relative_to(original_source)
    }
    files = {}
    for path in source.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"unexpected symlink in source tree: {path}")
        if path.is_file():
            relative = path.relative_to(source)
            snapshot.check(original_source / relative, path.read_bytes())
            files[relative] = path
    if expected != files.keys():
        missing = ", ".join(str(path) for path in sorted(expected - files.keys()))
        raise ValueError(f"source tree is incomplete: {source} ({missing})")
    return files


def checked_identifier(value):
    if not IDENTIFIER.fullmatch(value):
        raise ValueError(f"invalid Rust module or dependency name: {value}")
    return value


def transform(source, destination, package, xtask, *, module=None):
    destination.parent.mkdir(parents=True, exist_ok=True)
    if source.suffix != ".rs":
        shutil.copyfile(source, destination)
        return
    command = [
        xtask, "bundle-source", "--input", source, "--output", destination,
    ]
    for feature in sorted(package["features"]):
        command.extend(("--active-feature", feature))
    if module is not None:
        command.extend(("--module", module, "--strip-no-std"))
    for alias, target in sorted(package["local_dependencies"].items()):
        command.extend(("--dependency", f"{checked_identifier(alias)}={checked_identifier(target)}"))
    subprocess.run(command, check=True)


def copy_snapshot(snapshot, path, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(snapshot.read(path))


def stage_signer(snapshot, staging):
    directory = staging / "licenses/apple-codesign"
    copy_snapshot(snapshot, SIGNER_PATH / "LICENSE", directory / "LICENSE")
    copy_snapshot(snapshot, SIGNER_PATH / "README.md", directory / "README.md")
    with tarfile.open(directory / "source.tar", "w", format=tarfile.USTAR_FORMAT) as archive:
        for file in sorted(snapshot.blobs):
            if file.is_relative_to(SIGNER_PATH):
                contents = snapshot.read(file)
                member = tarfile.TarInfo((Path("apple-codesign") / file.relative_to(SIGNER_PATH)).as_posix())
                member.size = len(contents)
                archive.addfile(member, io.BytesIO(contents))


def attach_internal(path):
    source = path.read_text(encoding="utf-8")
    if path.name == "main.rs":
        # CLI fallback match arms still handle upstream non-exhaustive enums.
        source = "#![allow(unreachable_patterns)]\n\n" + source
    source += (
        "\n// Private modules retain APIs shared by the source workspace's other profiles.\n"
        "#[allow(dead_code, unused_imports, unreachable_patterns)]\n"
        "mod internal;\n"
    )
    path.write_text(source, encoding="utf-8", newline="\n")


def assemble(inventory, output, xtask, dmgez, hfsplus):
    if inventory["schema_version"] != 1:
        raise ValueError("unsupported registry dependency inventory version")
    packages = inventory["local"]
    cli_name = inventory["cli_package"]
    cli = packages[cli_name]
    composer_root = Path(cli["manifest_path"]).resolve().parents[2]
    modules = {name: checked_identifier(package["module"]) for name, package in packages.items()}
    if len(set(modules.values())) != len(modules):
        raise ValueError("local packages have conflicting Rust module names")
    defaults = ("src", "licenses", "examples", "LICENSE", "README.md", "source-provenance.json")
    if any((output / name).exists() for name in defaults):
        raise ValueError("standalone package output already contains generated source or documentation")
    if not (output / "Cargo.toml").is_file():
        raise ValueError("generate the registry package manifest before assembling its source")

    with tempfile.TemporaryDirectory(prefix="dmgez-registry-tree-", dir=output) as temporary:
        staging = Path(temporary)
        internal = staging / "src/internal"
        declarations = []
        for name, package in sorted(packages.items()):
            manifest = Path(package["manifest_path"]).resolve()
            source_path = Path(package["source_path"]).resolve()
            if source_path.parent != manifest.parent / "src":
                raise ValueError(f"unsupported source layout for {name}")
            if name in HFSPLUS_PACKAGES:
                snapshot = hfsplus
                original_source = HFSPLUS_PACKAGES[name] / "src"
            else:
                snapshot = dmgez
                original_source = manifest.parent.relative_to(composer_root) / "src"
            source = validated_sources(source_path, snapshot, original_source)
            if name == cli_name:
                for relative, path in sorted(source.items()):
                    transform(path, staging / "src" / relative, package, xtask)
                for root in ("lib.rs", "main.rs"):
                    attach_internal(staging / "src" / root)
            else:
                module = modules[name]
                if Path("lib.rs") not in source or source_path.name != "lib.rs":
                    raise ValueError(f"missing library entry point for {name}")
                for relative, path in sorted(source.items()):
                    transform(path, internal / module / "src" / relative, package, xtask, module=module)
                declarations.append(f'#[path = "{module}/src/lib.rs"]\npub(crate) mod {module};\n')
            if name == "apple-codesign":
                stage_signer(snapshot, staging)
        (internal / "mod.rs").write_text("".join(declarations), encoding="utf-8", newline="\n")
        copy_snapshot(dmgez, Path("LICENSE"), staging / "LICENSE")
        copy_snapshot(hfsplus, Path("LICENSE"), staging / "licenses/hfsplus/LICENSE")
        copy_snapshot(dmgez, README, staging / "README.md")
        for file in ("dmg.toml", "hello.txt"):
            copy_snapshot(dmgez, EXAMPLE / file, staging / EXAMPLE / file)
        provenance = {
            "dmgez_revision": dmgez.revision,
            "hfsplus_revision": hfsplus.revision,
            "active_features": {name: package["features"] for name, package in packages.items()},
            "files": {
                str(path.relative_to(staging).as_posix()): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in sorted(staging.rglob("*"))
                if path.is_file()
            },
        }
        (staging / "source-provenance.json").write_text(
            json.dumps(provenance, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n",
        )
        for name in defaults:
            (staging / name).rename(output / name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inventory", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--xtask", type=Path, required=True)
    parser.add_argument("--dmgez-root", type=Path, default=ROOT)
    parser.add_argument("--dmgez-revision", default="HEAD")
    parser.add_argument("--hfsplus-root", type=Path, default=ROOT.parent / "hfsplus")
    parser.add_argument("--hfsplus-revision", default=HFSPLUS_PIN)
    arguments = parser.parse_args()
    try:
        assemble(
            json.loads(arguments.inventory.read_text(encoding="utf-8")),
            arguments.output.resolve(), arguments.xtask.resolve(),
            Snapshot(arguments.dmgez_root, arguments.dmgez_revision),
            Snapshot(arguments.hfsplus_root, arguments.hfsplus_revision),
        )
    except (KeyError, OSError, TypeError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"{parser.prog}: {error}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
