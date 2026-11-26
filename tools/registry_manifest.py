#!/usr/bin/env python3
"""Describe a single-package registry bundle from a composed CLI build.

Capture metadata with Cargo's platform filter. Pair it with the selected CLI
tree from `cargo tree -p dmgez-cli --features inspect --target TARGET
-e normal,build --no-dedupe --prefix depth --format '{p}|{f}'`. Metadata alone
can include features activated by other workspace members.
"""

import argparse
from collections import defaultdict
from dataclasses import dataclass
import json
from pathlib import Path
import re
import tomllib


CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
TREE_ROW = re.compile(r"^(\d+)([\w-]+) v([^\s]+)(.*)\|([^|]*)$")
VALID_NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_-]*$")
LIB_KINDS = {"lib", "rlib"}
NORMAL_KIND = "normal"
BUILD_KIND = "build"


def identifier(name):
    if not VALID_NAME.fullmatch(name):
        raise ValueError(f"unsupported Cargo identifier {name!r}")
    return name.replace("-", "_")


@dataclass(frozen=True, order=True)
class Dependency:
    name: str
    version: str
    features: tuple[str, ...]


def read_tree(path):
    """Read the depth-prefixed, non-deduplicated tree for one CLI target."""
    ancestors = []
    packages = defaultdict(set)
    edges = defaultdict(set)
    for row in path.read_text(encoding="utf-8").splitlines():
        if not row.strip():
            continue
        matched = TREE_ROW.fullmatch(row)
        if not matched:
            raise ValueError(f"{path}: unexpected cargo tree row {row!r}")
        depth, name, version, annotation, feature_text = matched.groups()
        depth = int(depth)
        if depth > len(ancestors) or (packages and depth == 0) or " (*)" in annotation:
            raise ValueError(f"{path}: expected a single non-deduplicated cargo tree")
        key = name, version
        packages[key].add(tuple(filter(None, feature_text.split(","))))
        del ancestors[depth:]
        if ancestors:
            edges[ancestors[-1]].add(key)
        ancestors.append(key)
    if not packages:
        raise ValueError(f"{path}: empty cargo tree")
    return packages, edges, ancestors[0]


def read_lock(path):
    registry = {}
    with path.open("rb") as stream:
        for record in tomllib.load(stream)["package"]:
            if record.get("source") == CRATES_IO:
                key = record["name"], record["version"]
                checksum = record["checksum"]
                if not re.fullmatch(r"[0-9a-f]{64}", checksum):
                    raise ValueError(f"invalid registry checksum for {key}")
                registry[key] = checksum
    return registry


def selected_edges(node, packages, children):
    edges = []
    for dependency in node["deps"]:
        package = packages[dependency["pkg"]]
        key = package["name"], package["version"]
        if key not in children:
            continue
        kinds = {
            dep_kind["kind"] or NORMAL_KIND
            for dep_kind in dependency["dep_kinds"]
            if dep_kind["kind"] in (None, BUILD_KIND)
        }
        edges.extend((dependency["name"], key, kind) for kind in kinds)
    if {key for _, key, _ in edges} != children:
        raise ValueError(f"tree edges missing from Cargo metadata for {node['id']}")
    return sorted(edges)


def source_target(package, is_cli):
    expected = {"bin"} if is_cli else LIB_KINDS
    matches = [
        target for target in package["targets"]
        if expected.intersection(target["kind"]) and (not is_cli or target["name"] == "dmgez")
    ]
    if len(matches) != 1:
        raise ValueError(f"unable to identify a unique source target for {package['name']}")
    return matches[0]


def single_features(features, package):
    if len(features) != 1:
        raise ValueError(f"incompatible host and target feature sets for {package}")
    return next(iter(features))


def analyze_target(metadata_path, tree_path, lock, cli_package):
    metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    tree, edges, root = read_tree(tree_path)
    if root[0] != cli_package:
        raise ValueError(f"expected {cli_package} at the root of {tree_path}")
    packages = {package["id"]: package for package in metadata["packages"]}
    by_key = defaultdict(list)
    for package in packages.values():
        key = package["name"], package["version"]
        if key in tree:
            by_key[key].append(package)
    if set(by_key) != set(tree) or any(len(matches) != 1 for matches in by_key.values()):
        raise ValueError("tree package identities must match unique Cargo metadata packages")
    by_key = {key: matches[0] for key, matches in by_key.items()}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    local = {}
    direct = {NORMAL_KIND: {}, BUILD_KIND: {}}
    registry = []
    for key, package in sorted(by_key.items()):
        name, version = key
        if package["source"] == CRATES_IO:
            if key not in lock:
                raise ValueError(f"{name} {version} is missing from the composed registry lock")
            registry.append({
                "name": name, "version": version, "checksum": lock[key],
                "feature_sets": sorted(tree[key]),
            })
            continue
        if package["source"] is not None:
            raise ValueError(f"dependency {name} uses an unsupported source")
        if name in local:
            raise ValueError(f"more than one private source version for {name}")
        target = source_target(package, key == root)
        module = identifier(name)
        if not package["license"]:
            raise ValueError(f"private source package {name} has no SPDX license expression")
        entry = {
            "version": version,
            "manifest_path": package["manifest_path"],
            "source_path": target["src_path"],
            "crate_name": target["name"],
            "edition": target["edition"],
            "license": package["license"],
            "module": module,
            "features": single_features(tree[key], name),
            "declared_features": sorted(package["features"]),
            "local_dependencies": {},
        }
        for alias, child_key, kind in selected_edges(nodes[package["id"]], packages, edges[key]):
            alias = identifier(alias)
            child = by_key[child_key]
            if child["source"] is None:
                if kind != NORMAL_KIND:
                    raise ValueError(f"private build dependency {name} -> {child['name']}")
                previous = entry["local_dependencies"].setdefault(alias, identifier(child["name"]))
                if previous != identifier(child["name"]):
                    raise ValueError(f"conflicting private dependency alias {alias} in {name}")
            else:
                dependency = Dependency(*child_key, single_features(tree[child_key], child_key))
                previous = direct[kind].setdefault(alias, dependency)
                if previous != dependency:
                    raise ValueError(f"conflicting registry dependency alias {alias}")
        local[name] = entry
    for kind, dependencies in direct.items():
        if len({(dep.name, dep.version) for dep in dependencies.values()}) != len(dependencies):
            raise ValueError(f"repeated {kind} registry package with different aliases")
    return by_key[root], local, direct, registry


def render_dependencies(dependencies):
    for alias, dependency in sorted(dependencies.items()):
        # Cargo's SemVer requirements ignore build metadata. The lock and
        # inventory retain the full version string and registry checksum.
        details = {"version": f"={dependency.version.split('+', 1)[0]}"}
        if alias == identifier(dependency.name):
            alias = dependency.name
        else:
            details["package"] = dependency.name
        features = [feature for feature in dependency.features if feature != "default"]
        if "default" not in dependency.features:
            details["default-features"] = False
        if features:
            details["features"] = features
        body = ", ".join(f"{name} = {json.dumps(value)}" for name, value in details.items())
        yield f"{alias} = {{ {body} }}"


def render_manifest(root, local, direct):
    licenses = sorted({entry["license"] for entry in local.values()})
    compound_license = " AND ".join(
        f"({license})" if len(licenses) > 1 and " " in license else license
        for license in licenses
    )
    metadata = {
        "name": "dmgez",
        "version": root["version"],
        "edition": root["edition"],
        "rust-version": root["rust_version"],
        "description": root["description"],
        "license": compound_license,
        "repository": root["repository"],
        "readme": "README.md",
    }
    if any(not value for value in metadata.values()):
        raise ValueError("CLI release package metadata is incomplete")
    lines = [
        "[package]", *(f"{name} = {json.dumps(value)}" for name, value in metadata.items()),
        'resolver = "3"', "autoexamples = false", "autotests = false", "",
        "[lib]", 'path = "src/lib.rs"', "test = false", "doctest = false", "",
        "[[bin]]", 'name = "dmgez"', 'path = "src/main.rs"', "test = false", "",
        "[lints.rust]",
        'unexpected_cfgs = { level = "warn", check-cfg = ["cfg(kani)", "cfg(dmgez_loom)"] }',
        "",
    ]
    for kind, suffix in ((NORMAL_KIND, "dependencies"), (BUILD_KIND, "build-dependencies")):
        common = dict(next(iter(direct.values()))[kind])
        for selected in direct.values():
            common = {
                alias: value for alias, value in common.items() if selected[kind].get(alias) == value
            }
        if common:
            lines.extend((f"[{suffix}]", *render_dependencies(common), ""))
        for target, selected in sorted(direct.items()):
            specific = {
                alias: value for alias, value in selected[kind].items() if alias not in common
            }
            if specific:
                lines.extend((f"[target.{json.dumps(target)}.{suffix}]", *render_dependencies(specific), ""))
    return "\n".join(lines)


def parse_targets(arguments):
    targets = {}
    for argument in arguments:
        target, separator, path = argument.partition("=")
        if not separator or not VALID_NAME.fullmatch(target) or not path or target in targets:
            raise ValueError(f"expected a unique TARGET=PATH argument, got {argument!r}")
        targets[target] = Path(path)
    return targets


def generate(metadata, trees, lock, cli_package):
    if metadata.keys() != trees.keys():
        raise ValueError("Cargo metadata and tree inputs must describe the same targets")
    local = None
    root = None
    selected = {}
    inventory = {}
    for target in sorted(metadata):
        package, target_local, direct, registry = analyze_target(
            metadata[target], trees[target], lock, cli_package,
        )
        if root is None:
            root = package
            local = target_local
        elif package != root or target_local != local:
            raise ValueError("the private source graph differs between selected targets")
        selected[target] = direct
        inventory[target] = {
            "registry": registry,
            "direct": {
                kind: {alias: vars(dependency) for alias, dependency in dependencies.items()}
                for kind, dependencies in direct.items()
            },
        }
    if root is None:
        raise ValueError("at least one target is required")
    return render_manifest(root, local, selected), {
        "schema_version": 1, "cli_package": cli_package, "local": local, "targets": inventory,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metadata", action="append", required=True, metavar="TARGET=PATH")
    parser.add_argument("--tree", action="append", required=True, metavar="TARGET=PATH")
    parser.add_argument("--lockfile", type=Path, required=True)
    parser.add_argument("--package", default="dmgez-cli")
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--inventory", type=Path, required=True)
    args = parser.parse_args()
    try:
        manifest, inventory = generate(
            parse_targets(args.metadata), parse_targets(args.tree),
            read_lock(args.lockfile), args.package,
        )
        args.manifest.write_text(manifest, encoding="utf-8")
        args.inventory.write_text(json.dumps(inventory, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    except (KeyError, OSError, ValueError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
