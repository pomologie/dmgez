"""Regression checks for registry source snapshot preparation."""

from pathlib import Path
import subprocess
import tempfile
import unittest

from prepare_registry_package import extract_snapshot
from registry_source_tree import Snapshot


class SnapshotTests(unittest.TestCase):
    def test_archive_preserves_committed_bytes_with_crlf_checkout_enabled(self):
        with tempfile.TemporaryDirectory(prefix="dmgez-registry-snapshot-test-") as temporary:
            scratch = Path(temporary)
            repository = scratch / "source"
            repository.mkdir()

            def git(*arguments):
                subprocess.run(["git", "-C", repository, *arguments], check=True)

            git("init", "--quiet")
            git("config", "core.autocrlf", "false")
            path = Path("source.rs")
            data = b"mod nested;\n\n"
            (repository / path).write_bytes(data)
            git("add", "--", path)
            git(
                "-c", "user.name=Snapshot Test", "-c", "user.email=snapshot@example.invalid",
                "-c", "commit.gpgsign=false", "commit", "--quiet", "-m", "Record the source",
            )
            git("config", "core.autocrlf", "true")
            snapshot = Snapshot(repository, "HEAD")
            destination = scratch / "extracted"
            extract_snapshot(snapshot, destination, scratch / "source.tar")
            archived = (destination / path).read_bytes()
            self.assertEqual(archived, data)
            snapshot.check(path, archived)


if __name__ == "__main__":
    unittest.main()
