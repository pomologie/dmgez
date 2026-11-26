"""Check deterministic notice rendering and the draft packaging guard."""

from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

import package_cli
from release_notices import (
    NOTICE_DRAFT_HEADER, NOTICE_DRAFT_STATUS, NOTICE_HEADER,
    check_built_packages, notice_binary_digest, render,
)


TARGET = "aarch64-apple-darwin"
FIXTURE_DIGEST = "0" * 64


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
    return render(data, TARGET, "binary-hash", "lock-hash", ["inspect"], False)


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
            notices.write_text(NOTICE_DRAFT_HEADER + "\n", encoding="utf-8-sig")
            output = directory / "release.tar.gz"
            with self.assertRaisesRegex(ValueError, "requires review"):
                package_cli.package(source, TARGET, notices, output)
            self.assertFalse(output.exists())

    def test_generated_notice_requires_its_target_and_valid_digest(self):
        provenance = f"Target = {TARGET}\nCLI SHA-256 = {FIXTURE_DIGEST}"
        notice = f"{NOTICE_HEADER}\n{provenance}\n\nLicense body\n"
        self.assertEqual(notice_binary_digest(notice.replace("\n", "\r\n"), TARGET), FIXTURE_DIGEST)
        with self.assertRaisesRegex(ValueError, "target does not match"):
            notice_binary_digest(notice, "x86_64-apple-darwin")
        with self.assertRaisesRegex(ValueError, "requires a valid CLI SHA-256"):
            notice_binary_digest(notice.replace(FIXTURE_DIGEST, "invalid"), TARGET)
        with self.assertRaisesRegex(ValueError, "repeats the CLI SHA-256 field"):
            notice_binary_digest(f"{NOTICE_HEADER}\n{provenance}\nCLI SHA-256 = {FIXTURE_DIGEST}", TARGET)
        with self.assertRaisesRegex(ValueError, "requires review"):
            notice_binary_digest(f"{NOTICE_HEADER}\n{NOTICE_DRAFT_STATUS}\n{provenance}", TARGET)
        self.assertIsNone(notice_binary_digest("dmgez dependency notices\nManual notice text", TARGET))

    def test_packager_rejects_a_stale_notice_before_publishing(self):
        with TemporaryDirectory() as temporary:
            directory = Path(temporary)
            source = directory / "dmgez"
            source.write_bytes(b"a different executable fixture")
            notices = directory / "notices.txt"
            notices.write_text(
                f"{NOTICE_HEADER}\nTarget = {TARGET}\nCLI SHA-256 = {FIXTURE_DIGEST}\n",
                encoding="utf-8",
            )
            output = directory / "release.tar.gz"
            with self.assertRaisesRegex(ValueError, "does not match the staged binary"):
                package_cli.package(source, TARGET, notices, output)
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
