"""Exercise the POSIX installer with disposable release directories."""

import hashlib
import os
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import unittest


INSTALLER = Path(__file__).with_name("install.sh")
CLI = "dmgez"
MANIFEST = "SHA256SUMS"
ORIGINAL = b"first release bytes"
UPDATE = b"second release bytes"


@unittest.skipUnless(os.name == "posix", "POSIX install script")
class InstallTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory(prefix="dmgez-install-test.")
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        self.bundle = self.root / "release with spaces"
        self.bundle.mkdir()
        self.script = self.bundle / INSTALLER.name
        shutil.copyfile(INSTALLER, self.script)
        self.bin = self.root / "home with spaces" / ".local" / "bin"
        self.target = self.bin / CLI
        self.package(ORIGINAL)

    def package(self, data):
        (self.bundle / CLI).write_bytes(data)
        (self.bundle / MANIFEST).write_text(
            f"{hashlib.sha256(data).hexdigest()}  {CLI}\n",
            encoding="ascii",
        )

    def install(self, *args, directory=None):
        return subprocess.run(
            ["sh", self.script, "--bin-dir", directory or self.bin, *args],
            cwd=self.root,
            capture_output=True,
            text=True,
            check=False,
        )

    def assert_installed(self, data):
        self.assertEqual(self.target.read_bytes(), data)
        self.assertEqual(stat.S_IMODE(self.target.stat().st_mode), 0o755)
        self.assertEqual(list(self.bin.iterdir()), [self.target])

    def test_installs_and_handles_an_explicit_update(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_installed(ORIGINAL)
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Already installed", result.stdout)

        self.package(UPDATE)
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--replace", result.stderr)
        self.assert_installed(ORIGINAL)
        result = self.install("--replace")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_installed(UPDATE)

    def test_installs_to_a_selected_relative_directory(self):
        directory = Path("other bin")
        result = self.install(directory=directory)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / directory / CLI).read_bytes(), ORIGINAL)
        self.assertFalse(self.bin.exists())

    def test_declines_corrupt_data_before_creating_the_install_directory(self):
        (self.bundle / CLI).write_bytes(UPDATE)
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SHA-256 mismatch", result.stderr)
        self.assertFalse(self.bin.exists())

    def test_declines_duplicate_checksum_entries(self):
        manifest = self.bundle / MANIFEST
        line = manifest.read_text(encoding="ascii")
        manifest.write_text(line * 2, encoding="ascii")
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("expected one SHA-256 entry", result.stderr)
        self.assertFalse(self.bin.exists())

    def test_preserves_an_existing_symlink_or_directory(self):
        self.bin.mkdir(parents=True)
        existing = self.root / "existing binary"
        existing.write_bytes(UPDATE)
        self.target.symlink_to(existing)
        result = self.install("--replace")
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(self.target.is_symlink())
        self.assertEqual(existing.read_bytes(), UPDATE)

        self.target.unlink()
        self.target.mkdir()
        result = self.install("--replace")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(list(self.target.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
