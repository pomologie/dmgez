#!/usr/bin/env python3
"""Regression tests for local filesystem CLI composition."""

from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path, PureWindowsPath
import sys
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import compose_filesystem_cli as compose


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
            self.assertIn("Windows composition supports HFS+ project builds only", stderr.getvalue())
            resolve_packages.assert_not_called()

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
