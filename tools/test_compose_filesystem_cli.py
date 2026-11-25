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


class WindowsCompositionTests(unittest.TestCase):
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
            with patch.object(compose, "ROOT", root):
                compose.prepare_workspace(workspace, {compose.HFSPLUS_BUILDER: package})

            manifest = compose.load_toml(workspace / compose.MANIFEST)
            self.assertEqual(manifest["patch"]["crates-io"][compose.HFSPLUS_BUILDER]["path"], str(package))
            cli = compose.load_toml(workspace / compose.CLI / compose.MANIFEST)
            self.assertIn("project-build", cli["features"]["build"])

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
