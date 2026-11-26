import hashlib
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
import zipfile

from release_notices import NOTICE_DRAFT_HEADER, NOTICE_DRAFT_STATUS, NOTICE_HEADER, PACKAGE_HEADING
from release_sbom import CRATES_IO, archive_inventory, locked_components, notice_inventory


TARGET = "aarch64-apple-darwin"
LOCK_HASH = hashlib.sha256(b"composed lock fixture").hexdigest()
BINARY_HASH = hashlib.sha256(b"executable fixture").hexdigest()
SIGNER_HASH = hashlib.sha256(b"signer source fixture").hexdigest()
NOTICE = (
    f"{NOTICE_HEADER}\nTarget = {TARGET}\nCLI SHA-256 = {BINARY_HASH}\n"
    f"apple-codesign source SHA-256 = {SIGNER_HASH}\nComposed Cargo.lock SHA-256 = {LOCK_HASH}\n"
    f"\n{PACKAGE_HEADING}\nexample 1.0.0 | MIT OR Apache-2.0\n"
)


class ReleaseSbomTests(unittest.TestCase):
    def test_notice_target_lock_and_review_are_required(self):
        identity, _, inventory = notice_inventory(NOTICE, TARGET, LOCK_HASH)
        self.assertEqual(identity, (BINARY_HASH, SIGNER_HASH))
        self.assertEqual(inventory, {("example", "1.0.0"): "MIT OR Apache-2.0"})

        with self.assertRaisesRegex(ValueError, "digest differs"):
            notice_inventory(NOTICE, TARGET, BINARY_HASH)
        with self.assertRaisesRegex(ValueError, "target does not match"):
            notice_inventory(NOTICE, "x86_64-unknown-linux-gnu", LOCK_HASH)
        draft = NOTICE.replace(NOTICE_HEADER, f"{NOTICE_DRAFT_HEADER}\n{NOTICE_DRAFT_STATUS}", 1)
        with self.assertRaisesRegex(ValueError, "requires review"):
            notice_inventory(draft, TARGET, LOCK_HASH)

    def test_locked_inventory_only_includes_built_packages_and_preserves_source_identity(self):
        registry = {"name": "example", "version": "1.0.0", "source": CRATES_IO, "checksum": BINARY_HASH}
        vendored = {"name": "apple-codesign", "version": "0.29.0"}
        unused = {"name": "optional", "version": "1.0.0"}
        licenses = {("example", "1.0.0"): "MIT OR Apache-2.0", ("apple-codesign", "0.29.0"): "MPL-2.0"}
        components = locked_components({"package": [registry, vendored, unused]}, licenses)
        self.assertEqual(components.keys(), licenses.keys())
        self.assertEqual(components[("example", "1.0.0")]["purl"], "pkg:cargo/example@1.0.0")
        self.assertNotIn("purl", components[("apple-codesign", "0.29.0")])
        with self.assertRaisesRegex(ValueError, "absent from composed Cargo.lock"):
            locked_components({"package": [registry]}, licenses)
        with self.assertRaisesRegex(ValueError, "ambiguous locked package"):
            locked_components({"package": [registry, registry, vendored]}, licenses)

    def test_sbom_refuses_archive_with_changed_executable(self):
        target = "x86_64-pc-windows-msvc"
        prefix = f"dmgez-1.0.0-{target}"
        lock = b'[[package]]\nname = "dmgez-cli"\nversion = "1.0.0"\n'
        notice = NOTICE.replace(TARGET, target).replace(LOCK_HASH, hashlib.sha256(lock).hexdigest())
        notice = notice.replace("example 1.0.0", "dmgez-cli 1.0.0").replace(
            f"\n\n{PACKAGE_HEADING}",
            f"\nDefault features = enabled\nAdditional features = inspect\nLicense tool = cargo-about 0.9.3\n\n{PACKAGE_HEADING}",
        )
        with TemporaryDirectory() as temporary:
            archive = Path(temporary) / f"{prefix}.zip"
            with zipfile.ZipFile(archive, "w") as writer:
                writer.writestr(f"{prefix}/dmgez.exe", b"executable fixture")
                writer.writestr(f"{prefix}/THIRD_PARTY_NOTICES.txt", notice)
            bom = archive_inventory(archive, target, lock)
            self.assertEqual(bom["metadata"]["component"]["hashes"][0]["content"], BINARY_HASH)
            with zipfile.ZipFile(archive, "w") as writer:
                writer.writestr(f"{prefix}/dmgez.exe", b"changed executable fixture")
                writer.writestr(f"{prefix}/THIRD_PARTY_NOTICES.txt", notice)
            with self.assertRaisesRegex(ValueError, "archived CLI digest differs"):
                archive_inventory(archive, target, lock)


if __name__ == "__main__":
    unittest.main()
