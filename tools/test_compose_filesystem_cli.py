#!/usr/bin/env python3
"""Regression tests for local filesystem CLI composition."""

from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path, PureWindowsPath
import sys
from tempfile import TemporaryDirectory
import tomllib
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import compose_filesystem_cli as compose


class ReleaseCompositionTests(unittest.TestCase):
    def test_composition_preserves_the_manifest_groups(self):
        path = compose.ROOT / compose.CLI / compose.MANIFEST
        original = path.read_text(encoding="utf-8")
        self.assertEqual(compose.compose_cli_manifest(original, False, False, False), original)

        text = compose.compose_cli_manifest(original, reader=True, build=True, apfs=True)
        features = tomllib.loads(text)["features"]
        self.assertIn(compose.PROJECT_RELEASE_FEATURE, features["default"])
        self.assertIn("signing-rust", features["default"])
        self.assertIn("dmgez-runtime/project-build", features["project-build"])
        for line in original.splitlines():
            if line.startswith("# "):
                self.assertIn(line, text.splitlines())
        for group in (
            "HFS+ project build, edit and release", "HFS+ Finder metadata", "HFS+ volume access",
            "APFS volume access", "Composed filesystem dependencies", "Hashes and checksums",
        ):
            self.assertIn(f"\n# {group}\n", text)
        self.assertIn('image-edit = [\n\t"project-build",\n', text)
        self.assertIn('[dev-dependencies]\n# Workspace presentation\ndmgez-finder.workspace = true\n', text)
        composed = tomllib.loads(text)["dependencies"]
        for name, group in compose.COMPOSED_CRATE_GROUPS.items():
            self.assertEqual(composed[name]["path"], f"../../crates/{group}/{name}")

    def test_watch_requires_selected_mount_and_installation_services(self):
        manifest = (compose.ROOT / compose.CLI / compose.MANIFEST).read_text(encoding="utf-8")
        for reader in (False, True):
            with self.subTest(reader=reader):
                text = compose.compose_cli_manifest(manifest, reader=reader, build=False, apfs=False)
                features = tomllib.loads(text)["features"]
                self.assertNotIn(compose.MOUNT_WATCH_FEATURE, features["default"])
                self.assertEqual(compose.MOUNT_WATCH_FEATURE in features, reader)
                if reader:
                    self.assertEqual(
                        features[compose.MOUNT_WATCH_FEATURE],
                        [compose.INSTALL_FEATURE, "mount-system", "dmgez-mount/watch", "dmgez-runtime/mount-watch"],
                    )

    def test_partition_writing_is_an_explicit_project_build_feature(self):
        manifest = (compose.ROOT / compose.CLI / compose.MANIFEST).read_text(encoding="utf-8")
        for build in (False, True):
            with self.subTest(build=build):
                text = compose.compose_cli_manifest(manifest, reader=False, build=build, apfs=False)
                features = tomllib.loads(text)["features"]
                self.assertNotIn(compose.PARTITION_WRITE_FEATURE, features["default"])
                self.assertEqual(compose.PARTITION_WRITE_FEATURE in features, build)
                if build:
                    self.assertEqual(
                        features[compose.PARTITION_WRITE_FEATURE],
                        ["project-build", f"{compose.ADAPTER}/partition-write"],
                    )

    def test_installer_test_uses_only_the_composed_filesystem_readers(self):
        manifest = (compose.ROOT / compose.CLI / compose.MANIFEST).read_text(encoding="utf-8")
        for reader in (False, True):
            for apfs in (False, True):
                with self.subTest(reader=reader, apfs=apfs):
                    text = compose.compose_cli_manifest(manifest, reader=reader, build=True, apfs=apfs)
                    features = tomllib.loads(text)["features"][compose.INSTALLER_TEST_FEATURE]
                    self.assertEqual(compose.READ_FEATURE in features, reader)
                    self.assertEqual(compose.APFS_FEATURE in features, apfs)

    def test_release_forwards_install_only_when_the_hfsplus_reader_is_present(self):
        manifest = (compose.ROOT / compose.CLI / compose.MANIFEST).read_text(encoding="utf-8")
        for reader in (False, True):
            with self.subTest(reader=reader):
                text = compose.compose_cli_manifest(manifest, reader=reader, build=True, apfs=False)
                features = tomllib.loads(text)["features"]
                self.assertEqual(compose.INSTALL_FEATURE in features[compose.PROJECT_RELEASE_FEATURE], reader)

    def test_image_edit_requires_both_local_hfsplus_engines(self):
        manifest = (compose.ROOT / compose.CLI / compose.MANIFEST).read_text(encoding="utf-8")
        for reader in (False, True):
            for build in (False, True):
                with self.subTest(reader=reader, build=build):
                    text = compose.compose_cli_manifest(manifest, reader=reader, build=build, apfs=False)
                    composed = tomllib.loads(text)
                    features = composed["features"]
                    self.assertEqual(compose.IMAGE_EDIT_FEATURE in features, reader and build)
                    self.assertEqual("dmgez-finder" in composed["dev-dependencies"], reader and build)
                    self.assertNotIn("dmgez-finder", composed["dependencies"])
                    if reader and build:
                        self.assertIn("dmgez-operations/image-edit", features[compose.IMAGE_EDIT_FEATURE])

    def test_reader_only_composition_resolves_without_a_builder(self):
        root = Path("local")
        args = ["compose_filesystem_cli.py", "--hfsplus", str(root),
                "--no-default-features", "--features", compose.READ_FEATURE]
        packages = {compose.HFSPLUS_READER: root}
        with (
            patch.object(sys, "argv", args),
            patch.object(sys, "platform", "linux"),
            patch.object(compose, "resolve_hfsplus", return_value=packages) as resolve,
            patch.object(compose, "prepare_workspace"),
            patch.object(compose, "resolve_lock"),
            patch.object(compose, "run_cli"),
        ):
            self.assertEqual(compose.main(), 0)
        resolve.assert_called_once_with(root, None, reader=True, build=False)

    def test_actual_preview_requires_the_hfsplus_reader_and_design_renderer(self):
        manifest = (compose.ROOT / compose.CLI / compose.MANIFEST).read_text(encoding="utf-8")
        for reader in (False, True):
            with self.subTest(reader=reader):
                text = compose.compose_cli_manifest(manifest, reader=reader, build=False, apfs=False)
                features = tomllib.loads(text)["features"]
                self.assertEqual(compose.ACTUAL_PREVIEW_FEATURE in features, reader)
                if reader:
                    self.assertEqual(features[compose.ACTUAL_PREVIEW_FEATURE], [
                        "preview", compose.READ_FEATURE, "dmgez-runtime/actual-preview",
                    ])


class DesktopCompositionTests(unittest.TestCase):
    def test_injects_the_optional_hfsplus_browse_feature(self):
        original = (compose.ROOT / compose.DESKTOP / compose.MANIFEST).read_text(encoding="utf-8")
        base = tomllib.loads(original)
        manifest = tomllib.loads(compose.compose_desktop_manifest(original))
        self.assertEqual(manifest["features"], {
            **base.get("features", {}),
            compose.READ_FEATURE: [f"dep:{compose.ADAPTER}", "dep:dmgez-vfs"],
            compose.ACTUAL_PREVIEW_FEATURE: [compose.READ_FEATURE, "dmgez-runtime/actual-preview"],
        })
        self.assertEqual(manifest["dependencies"][compose.ADAPTER]["features"], ["browse"])
        self.assertTrue(manifest["dependencies"][compose.ADAPTER]["optional"])
        self.assertEqual(manifest["dev-dependencies"], base["dev-dependencies"])

    def test_desktop_project_build_uses_the_composed_runtime(self):
        original = (compose.ROOT / compose.DESKTOP / compose.MANIFEST).read_text(encoding="utf-8")
        runtime = (compose.ROOT / compose.RUNTIME / compose.MANIFEST).read_text(encoding="utf-8")
        manifest = tomllib.loads(compose.compose_desktop_manifest(original, build=True))
        self.assertEqual(manifest["features"][compose.PROJECT_BUILD_FEATURE], [
            "dmgez-runtime/project-build", "dmgez-runtime/finder-write", "dmgez-runtime/hfsplus-parallel",
        ])
        self.assertEqual(manifest["dependencies"][compose.ADAPTER]["features"], ["browse"])
        features = tomllib.loads(compose.compose_runtime_manifest(runtime, reader=True, build=True, apfs=False))[
            "features"
        ]
        self.assertIn(f"{compose.ADAPTER}?/finder-write", features["finder-write"])
        self.assertEqual(features["hfsplus-parallel"], [f"{compose.ADAPTER}/parallel"])

    def test_resolves_and_tests_only_the_desktop_reader_on_each_host(self):
        root = Path("local")
        args = ["compose_filesystem_cli.py", "--hfsplus", str(root), "--desktop", "--test"]
        packages = {compose.HFSPLUS_READER: root}
        for platform in ("darwin", "linux", "win32"):
            with (
                self.subTest(platform=platform),
                patch.object(sys, "argv", args),
                patch.object(sys, "platform", platform),
                patch.object(compose, "resolve_hfsplus", return_value=packages) as resolve,
                patch.object(compose, "prepare_workspace") as prepare,
                patch.object(compose, "resolve_lock"),
                patch.object(compose, "run_desktop") as run,
                patch.object(compose, "run_cli") as cli,
            ):
                self.assertEqual(compose.main(), 0)
                resolve.assert_called_once_with(root, None, reader=True, build=False)
                _, workspace, options, resolved = run.call_args.args
                prepare.assert_called_once_with(workspace, packages, desktop=True)
                self.assertEqual(resolved, packages)
                self.assertTrue(options.test)
                self.assertEqual(options.target_dir.name, "hfsplus-desktop")
                cli.assert_not_called()

    def test_qualified_project_build_feature_resolves_desktop_writer(self):
        root = Path("local")
        feature = f"{compose.DESKTOP_PACKAGE}/{compose.PROJECT_BUILD_FEATURE}"
        args = ["compose_filesystem_cli.py", "--hfsplus", str(root), "--desktop", "--features", feature]
        packages = {compose.HFSPLUS_READER: root, compose.HFSPLUS_BUILDER: root}
        with (
            patch.object(sys, "argv", args),
            patch.object(compose, "resolve_hfsplus", return_value=packages) as resolve,
            patch.object(compose, "prepare_workspace"),
            patch.object(compose, "resolve_lock"),
            patch.object(compose, "run_desktop"),
        ):
            self.assertEqual(compose.main(), 0)
            resolve.assert_called_once_with(root, None, reader=True, build=True)

    def test_test_mode_selects_the_desktop_binary(self):
        directory = Path.cwd()
        options = SimpleNamespace(
            test=True, target_dir=directory, release=False, no_default_features=False, features=[],
        )
        with patch.object(compose.subprocess, "run") as run:
            compose.run_desktop(["cargo"], directory, options, {compose.HFSPLUS_READER: directory})
        command = run.call_args.args[0]
        self.assertEqual(command[:5], ["cargo", "test", "--frozen", "--package", compose.DESKTOP_PACKAGE])
        self.assertEqual(command[-4:], ["--features", compose.READ_FEATURE, "--bin", compose.DESKTOP_PACKAGE])


class WindowsCompositionTests(unittest.TestCase):
    def test_fetch_preloads_the_checked_in_lockfile_before_composition(self):
        root = Path("sibling hfsplus")
        args = ["compose_hfsplus_cli.py", "--hfsplus", str(root), "--fetch", "--release"]
        packages = {compose.HFSPLUS_BUILDER: root}
        with (
            patch.object(sys, "argv", args),
            patch.object(sys, "platform", "win32"),
            patch.object(compose, "resolve_hfsplus", return_value=packages),
            patch.object(compose.subprocess, "run") as run,
            patch.object(compose, "prepare_workspace") as prepare,
            patch.object(compose, "resolve_lock") as resolve_lock,
            patch.object(compose, "run_cli"),
        ):
            prepare.side_effect = lambda *_: run.assert_called_once()
            self.assertEqual(compose.main(), 0)

        cargo, workspace, resolved = resolve_lock.call_args.args
        run.assert_called_once_with([*cargo, "fetch", "--locked"], cwd=compose.ROOT, check=True)
        self.assertEqual(resolved, packages)
        self.assertNotEqual(workspace, compose.ROOT)

    def test_windows_composes_the_hfsplus_builder_for_local_project_tests(self):
        source = Path("Unpacked HFS+ packages")
        args = ["compose_hfsplus_cli.py", "--hfsplus-packages", str(source), "--test"]
        packages = {compose.HFSPLUS_BUILDER: source}
        with (
            patch.object(sys, "argv", args),
            patch.object(sys, "platform", "win32"),
            patch.object(compose, "resolve_hfsplus", return_value=packages) as resolve_hfsplus,
            patch.object(compose, "prepare_workspace") as prepare_workspace,
            patch.object(compose, "resolve_lock") as resolve_lock,
            patch.object(compose, "run_cli") as run_cli,
        ):
            self.assertEqual(compose.main(), 0)

        resolve_hfsplus.assert_called_once_with(None, source, reader=False, build=True)
        cargo, workspace, options, resolved = run_cli.call_args.args
        prepare_workspace.assert_called_once_with(workspace, packages)
        resolve_lock.assert_called_once_with(cargo, workspace, packages)
        self.assertEqual(resolved, packages)
        self.assertTrue(options.test)

    def test_windows_rejects_unqualified_reader_profiles_before_resolution(self):
        profiles = [
            ["--apfs", "repo"],
            ["--hfsplus", "repo", "--features", "hfsplus-verify"],
            ["--hfsplus", "repo", "--features", "dmgez-cli/hfsplus-extract"],
        ]
        for profile in profiles:
            with (
                self.subTest(profile=profile),
                patch.object(sys, "argv", ["compose_filesystem_cli.py", *profile]),
                patch.object(sys, "platform", "win32"),
                patch.object(compose, "resolve_packages") as resolve_packages,
                redirect_stderr(StringIO()) as stderr,
                self.assertRaises(SystemExit) as error,
            ):
                compose.main()
            self.assertEqual(error.exception.code, 2)
            self.assertIn("Windows composition supports HFS+ project builds and image editing only", stderr.getvalue())
            resolve_packages.assert_not_called()

    def test_windows_image_edit_resolves_both_engines_without_other_reader_profiles(self):
        root = Path("local")
        args = ["compose_filesystem_cli.py", "--hfsplus", str(root), "--features", compose.IMAGE_EDIT_FEATURE]
        packages = {compose.HFSPLUS_BUILDER: root, compose.HFSPLUS_READER: root}
        with (
            patch.object(sys, "argv", args),
            patch.object(sys, "platform", "win32"),
            patch.object(compose, "resolve_hfsplus", return_value=packages) as resolve,
            patch.object(compose, "prepare_workspace"),
            patch.object(compose, "resolve_lock"),
            patch.object(compose, "run_cli"),
        ):
            self.assertEqual(compose.main(), 0)
        resolve.assert_called_once_with(root, None, reader=True, build=True)

    def test_workspace_round_trips_drive_prefix_backslashes_spaces_and_unicode(self):
        package = PureWindowsPath(r"\\?\C:\Build assets\Uñpacked 💿\hfsplus-builder-0.1.0")
        with TemporaryDirectory() as temporary:
            base = Path(temporary)
            root = base / "source"
            workspace = base / "composed"
            workspace.mkdir()
            for directory in compose.SOURCE_DIRS:
                (root / directory).mkdir(parents=True)
            (root / compose.CLI).mkdir()
            for file in (compose.LOCK, compose.TOOLCHAIN, "rustfmt.toml", "LICENSE"):
                (root / file).write_text("", encoding="utf-8")
            (root / compose.MANIFEST).write_text("[workspace]\n", encoding="utf-8")
            original = compose.ROOT / compose.CLI / compose.MANIFEST
            (root / compose.CLI / compose.MANIFEST).write_bytes(original.read_bytes())
            (root / compose.RUNTIME).mkdir(parents=True)
            runtime = compose.ROOT / compose.RUNTIME / compose.MANIFEST
            (root / compose.RUNTIME / compose.MANIFEST).write_bytes(runtime.read_bytes())
            with patch.object(compose, "ROOT", root):
                compose.prepare_workspace(workspace, {compose.HFSPLUS_BUILDER: package})

            manifest = compose.load_toml(workspace / compose.MANIFEST)
            self.assertEqual(manifest["patch"]["crates-io"][compose.HFSPLUS_BUILDER]["path"], str(package))
            cli = compose.load_toml(workspace / compose.CLI / compose.MANIFEST)
            self.assertIn("project-build", cli["features"]["build"])
            runtime = compose.load_toml(workspace / compose.RUNTIME / compose.MANIFEST)
            self.assertIn("dmgez-hfsplus/build", runtime["features"]["project-build"])
            self.assertEqual(
                runtime["dependencies"][compose.ADAPTER]["path"], "../../adapters/dmgez-hfsplus",
            )

    def test_windows_test_mode_runs_native_ntfs_regression(self):
        directory = Path.cwd()
        options = SimpleNamespace(
            test=True, target_dir=directory, release=False, no_default_features=False, features=[],
        )
        with (
            patch.object(sys, "platform", "win32"),
            patch.object(compose.subprocess, "run") as run,
        ):
            compose.run_cli(["cargo"], directory, options, {compose.HFSPLUS_BUILDER: directory})

        command = run.call_args.args[0]
        self.assertEqual(command[-4:], ["--test", compose.CLI_TEST, "--test", compose.WINDOWS_CLI_TEST])


if __name__ == "__main__":
    unittest.main()
