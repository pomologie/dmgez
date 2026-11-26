"""Check deterministic notice rendering and the draft packaging guard."""

from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

import package_cli
from release_notices import (
    NOTICE_DRAFT_HEADER, NOTICE_DRAFT_STATUS, NOTICE_HEADER,
    SIGNER_ARCHIVE, SIGNER_SHA256_FIELD, SUPPLEMENTAL_NOTICES,
    check_built_packages, digest, notice_digests, render, signer_digest,
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
    return render(data, TARGET, "binary-hash", "lock-hash", ["inspect"], False, signer_hash="source-hash")


def reviewed_notice(target, binary_hash, signer_hash):
    return (
        f"{NOTICE_HEADER}\nTarget = {target}\nCLI SHA-256 = {binary_hash}\n"
        f"{SIGNER_SHA256_FIELD} = {signer_hash}\n"
    )


class ReleaseNoticeTests(unittest.TestCase):
    def test_preserves_source_text_and_labels_canonical_fallback(self):
        data = report()
        original = draft(data)
        data["crates"].reverse()
        data["licenses"].reverse()
        self.assertEqual(draft(data), original)
        self.assertIn("CLI SHA-256 = binary-hash", original)
        self.assertIn(f"{SIGNER_SHA256_FIELD} = source-hash", original)
        self.assertIn("Copyright & license\n", original)
        self.assertIn("Canonical license texts requiring source notice review\nsecond 2.0.0 | MIT", original)
        self.assertNotIn("/private/source", original)

    def test_incomplete_inventory_fails(self):
        data = report()
        data["licenses"].pop()
        with self.assertRaisesRegex(ValueError, "do not cover"):
            draft(data)

    def test_supplemental_notices_use_the_reported_package_source(self):
        with TemporaryDirectory() as temporary:
            directory = Path(temporary)
            data = report()
            package = data["crates"][0]["package"]
            package.update(name="ring", version="0.17.14", manifest_path=str(directory / "Cargo.toml"))
            paths = SUPPLEMENTAL_NOTICES["ring", "0.17.14"]
            for relative in paths:
                notice = directory / relative
                notice.parent.mkdir(parents=True, exist_ok=True)
                notice.write_text(f"Source notice from {relative}\n", encoding="utf-8")
            text = draft(data)
            for relative in paths:
                self.assertIn(f"ring 0.17.14 | {relative}\n\nSource notice from {relative}\n", text)
            self.assertNotIn(str(directory), text)
            (directory / paths[-1]).unlink()
            with self.assertRaises(FileNotFoundError):
                draft(data)

    def test_notice_inventory_matches_the_packages_cargo_built(self):
        check_built_packages(report(), {"first-id", "second-id"})
        with self.assertRaisesRegex(ValueError, "missing other-id, extra second-id"):
            check_built_packages(report(), {"first-id", "other-id"})

    def test_draft_recovers_the_upstream_binrw_attribution(self):
        data = report()
        for entry, name in zip(data["crates"], ("binrw", "binrw_derive")):
            entry["package"].update(name=name, version="0.15.2")
        text = draft(data)
        for name in ("binrw", "binrw_derive"):
            self.assertIn(
                f"{name} 0.15.2 | upstream LICENSE\n\nMIT License\n\n"
                "Copyright (c) jam1garner and other contributors",
                text,
            )

    def test_supplemental_notice_retains_only_the_source_copyright_header(self):
        with TemporaryDirectory() as temporary:
            directory = Path(temporary)
            data = report()
            package = data["crates"][0]["package"]
            package.update(name="wtf-string", version="0.1.0", manifest_path=str(directory / "Cargo.toml"))
            source = directory / "src/lib.rs"
            source.parent.mkdir()
            source.write_text("// Copyright (c) 2026 Mike Grier\nfn private_code() {}\n", encoding="utf-8")
            notice = draft(data)
            self.assertIn("wtf-string 0.1.0 | src/lib.rs:1\n\nCopyright (c) 2026 Mike Grier", notice)
            self.assertNotIn("private_code", notice)
            source.write_text("fn private_code() {}\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "lacks a copyright header"):
                draft(data)

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

    def test_generated_notice_requires_its_target_and_valid_digests(self):
        provenance = reviewed_notice(TARGET, FIXTURE_DIGEST, FIXTURE_DIGEST)
        notice = f"{provenance}\nLicense body\n"
        self.assertEqual(notice_digests(notice.replace("\n", "\r\n"), TARGET), (FIXTURE_DIGEST, FIXTURE_DIGEST))
        with self.assertRaisesRegex(ValueError, "target does not match"):
            notice_digests(notice, "x86_64-apple-darwin")
        with self.assertRaisesRegex(ValueError, "requires a valid CLI SHA-256"):
            notice_digests(reviewed_notice(TARGET, "invalid", FIXTURE_DIGEST), TARGET)
        with self.assertRaisesRegex(ValueError, "requires a valid apple-codesign source SHA-256"):
            notice_digests(provenance.rsplit(SIGNER_SHA256_FIELD, 1)[0], TARGET)
        with self.assertRaisesRegex(ValueError, "repeats the CLI SHA-256 field"):
            notice_digests(f"{provenance}CLI SHA-256 = {FIXTURE_DIGEST}", TARGET)
        with self.assertRaisesRegex(ValueError, "requires review"):
            notice_digests(f"{provenance}{NOTICE_DRAFT_STATUS}\n", TARGET)
        self.assertIsNone(notice_digests("dmgez dependency notices\nManual notice text", TARGET))

    def test_packager_rejects_a_stale_notice_before_publishing(self):
        with TemporaryDirectory() as temporary:
            directory = Path(temporary)
            source = directory / "dmgez"
            source.write_bytes(b"a different executable fixture")
            notices = directory / "notices.txt"
            notices.write_text(
                reviewed_notice(TARGET, FIXTURE_DIGEST, FIXTURE_DIGEST),
                encoding="utf-8",
            )
            output = directory / "release.tar.gz"
            with self.assertRaisesRegex(ValueError, "does not match the staged binary"):
                package_cli.package(source, TARGET, notices, output)
            self.assertFalse(output.exists())

    def test_packager_binds_the_archived_signer_source_to_the_notice(self):
        with TemporaryDirectory() as temporary:
            directory = Path(temporary)
            binary = directory / "executable"
            binary.write_bytes(b"test executable")
            source = directory / "lib.rs"
            source.write_bytes(b"original signer")
            member = SIGNER_ARCHIVE + "src/lib.rs"
            signer_hash = signer_digest({member: digest(source)})
            notices = directory / "notices.txt"
            for target in (TARGET, "x86_64-pc-windows-msvc"):
                with self.subTest(target=target):
                    notices.write_text(reviewed_notice(target, digest(binary), signer_hash), encoding="utf-8")
                    suffix = package_cli.archive_format(target)
                    output = directory / f"release{suffix}"
                    with (
                        patch.object(package_cli, "validate_machine"),
                        patch.object(package_cli, "validate_binary"),
                        patch.object(package_cli, "signer_members", return_value={member: source}),
                    ):
                        source.write_bytes(b"changed signer")
                        with self.assertRaisesRegex(ValueError, "source SHA-256 does not match the archive"):
                            package_cli.package(binary, target, notices, output)
                        self.assertFalse(output.exists())
                        source.write_bytes(b"original signer")
                        self.assertEqual(package_cli.package(binary, target, notices, output), digest(output))


if __name__ == "__main__":
    unittest.main()
