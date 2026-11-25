#!/usr/bin/env python3
"""Exercise the production CLI dependency guard against a real Cargo mutation."""

import os
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory
import unittest

import compose_filesystem_cli as compose


class ArchitectureMutationTests(unittest.TestCase):
    def test_rejects_a_gui_runtime_added_to_the_cli(self):
        with TemporaryDirectory(prefix="dmgez-architecture-mutation.") as temporary:
            workspace = Path(temporary)
            compose.prepare_workspace(workspace, {})
            manifest = workspace / compose.CLI / compose.MANIFEST
            source = manifest.read_text(encoding="utf-8")
            section = "[dependencies]\n"
            self.assertEqual(source.count(section), 1)
            manifest.write_text(
                source.replace(section, section + "eframe.workspace = true\n"), encoding="utf-8",
            )
            compose.resolve_lock(["cargo"], workspace, {})
            guard = workspace / "tools" / "check_architecture.py"
            guard.parent.mkdir()
            shutil.copyfile(compose.ROOT / "tools" / guard.name, guard)
            result = subprocess.run(
                [sys.executable, "-B", guard], cwd=workspace,
                env=os.environ | {"CARGO_NET_OFFLINE": "true"},
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            for profile in ("minimal", "serial", "default"):
                self.assertIn(f"{profile} profile: forbidden dependencies", result.stderr)
            self.assertIn("'eframe'", result.stderr)


if __name__ == "__main__":
    unittest.main()
