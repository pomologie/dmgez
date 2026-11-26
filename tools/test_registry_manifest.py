"""Regression tests for standalone registry dependency planning."""

import json
from pathlib import Path
import tempfile
import tomllib
import unittest

import registry_manifest as manifest


def package(name, *, registry=False, executable=False):
    kind = "bin" if executable else "lib"
    return {
        "id": name, "name": name, "version": "0.1.0",
        "source": manifest.CRATES_IO if registry else None,
        "manifest_path": f"/workspace/{name}/Cargo.toml",
        "edition": "2024", "rust_version": "1.98",
        "description": "DMG tools", "license": "Apache-2.0",
        "repository": "https://github.com/pomologie/dmgez",
        "features": {"used": [], "unused": []},
        "targets": [{
            "name": "dmgez" if executable else name.replace("-", "_"),
            "kind": [kind], "edition": "2024",
            "src_path": f"/workspace/{name}/src/{kind}.rs",
        }],
    }


def dependency(alias, package, kind=None):
    return {"name": alias, "pkg": package, "dep_kinds": [{"kind": kind, "target": None}]}


class RegistryManifestTests(unittest.TestCase):
    def test_uses_selected_tree_and_preserves_platform_dependencies(self):
        root = package("dmgez-cli", executable=True)
        helper = package("private-code")
        helper["license"] = "MPL-2.0"
        data = {
            "packages": [
                root, helper,
                *(package(name, registry=True) for name in ("quick-xml", "other", "inactive")),
            ],
            "resolve": {"nodes": [
                {"id": root["id"], "deps": [
                    dependency("private_alias", helper["id"]), dependency("inactive", "inactive"),
                ]},
                {"id": helper["id"], "deps": [
                    dependency("quick_xml", "quick-xml"), dependency("renamed", "other"),
                ]},
            ]},
        }
        lock = {(name, "0.1.0"): "a" * 64 for name in ("quick-xml", "other")}
        base = "0dmgez-cli v0.1.0|used\n1private-code v0.1.0|\n2quick-xml v0.1.0|default,extra\n"
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            metadata = directory / "metadata.json"
            metadata.write_text(json.dumps(data), encoding="utf-8")
            linux_tree = directory / "linux.tree"
            windows_tree = directory / "windows.tree"
            linux_tree.write_text(base, encoding="utf-8")
            windows_tree.write_text(base + "2other v0.1.0|\n", encoding="utf-8")
            linux = "x86_64-unknown-linux-gnu"
            windows = "x86_64-pc-windows-msvc"
            rendered, inventory = manifest.generate(
                {linux: metadata, windows: metadata}, {linux: linux_tree, windows: windows_tree},
                lock, root["name"],
            )

        cargo = tomllib.loads(rendered)
        self.assertEqual(cargo["package"]["name"], "dmgez")
        self.assertEqual(cargo["package"]["license"], "Apache-2.0 AND MPL-2.0")
        self.assertEqual(cargo["lib"]["path"], "src/lib.rs")
        self.assertEqual(cargo["bin"][0]["name"], "dmgez")
        self.assertEqual(cargo["features"]["default"], ["bundle_dmgez_cli_used"])
        self.assertNotIn("bundle_dmgez_cli_unused", cargo["features"])
        self.assertEqual(cargo["dependencies"], {
            "quick-xml": {"version": "=0.1.0", "features": ["extra"]},
        })
        self.assertEqual(cargo["target"][windows]["dependencies"], {
            "renamed": {"version": "=0.1.0", "package": "other", "default-features": False},
        })
        self.assertEqual(inventory["local"]["dmgez-cli"]["local_dependencies"], {
            "private_alias": "private_code",
        })
        self.assertNotIn("inactive", repr(inventory))

    def test_rejects_an_unpaired_target(self):
        with self.assertRaisesRegex(ValueError, "same targets"):
            manifest.generate({"x86_64-unknown-linux-gnu": Path("metadata")}, {}, {}, "dmgez-cli")


if __name__ == "__main__":
    unittest.main()
