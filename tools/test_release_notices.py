"""Check deterministic notice rendering and the draft packaging guard."""

from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

import package_cli
from release_notices import NOTICE_DRAFT_HEADER, check_built_packages, render


def report():
    first = {"id": "first-id", "name": "first", "version": "1.0.0"}
    second = {"id": "second-id", "name": "second", "version": "2.0.0"}
    return {
        "crates": [
            {"package": second, "license": "MIT"},
            {"package": first, "license": "MIT"},
        ],
        "licenses": [
            {"id": "MIT", "text": "Canonical text\n", "source_path": None, "used_by": [{"crate": second}]},
            {"id": "MIT", "text": "Copyright & license\n", "source_path": "/private/source/LICENSE",
             "used_by": [{"crate": first}]},
        ],
    }


def draft(data):
    return render(data, "aarch64-apple-darwin", "binary-hash", "lock-hash", ["inspect"], False)


class ReleaseNoticeTests(unittest.TestCase):
    def test_preserves_source_text_and_labels_canonical_fallback(self):
        data = report()
        original = draft(data)
        data["crates"].reverse()
        data["licenses"].reverse()
        self.assertEqual(draft(data), original)
        self.assertIn("CLI SHA-256 = binary-hash", original)
        self.assertIn("Copyright & license\n", original)
        self.assertIn("Canonical license texts requiring source notice review\nsecond 2.0.0 | MIT", original)
        self.assertNotIn("/private/source", original)

    def test_incomplete_inventory_fails(self):
        data = report()
        data["licenses"].pop()
        with self.assertRaisesRegex(ValueError, "do not cover"):
            draft(data)

    def test_notice_inventory_matches_the_packages_cargo_built(self):
        check_built_packages(report(), {"first-id", "second-id"})
        with self.assertRaisesRegex(ValueError, "missing other-id, extra second-id"):
            check_built_packages(report(), {"first-id", "other-id"})

    def test_packager_refuses_an_unreviewed_draft(self):
        with TemporaryDirectory() as temporary:
            directory = Path(temporary)
            source = directory / "dmgez"
            source.write_bytes(b"test fixture")
            notices = directory / "notices.txt"
            notices.write_text(NOTICE_DRAFT_HEADER + "\n", encoding="utf-8")
            output = directory / "release.tar.gz"
            with self.assertRaisesRegex(ValueError, "requires review"):
                package_cli.package(source, "aarch64-apple-darwin", notices, output)
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
