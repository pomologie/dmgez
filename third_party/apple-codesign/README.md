# Vendored apple-codesign

This package contains the offline ad hoc DMG signer from [apple-codesign 0.29.0](https://github.com/indygreg/apple-platform-rs/tree/apple-codesign%2F0.29.0/apple-codesign). The crates.io archive `apple-codesign-0.29.0.crate` has SHA-256 `f24e9ebdb70a2aee3ca1cea217009fb50776955f0d7678c31d22e48c1524667f` and records upstream commit `2312b1e5bd77322c019613e0ca624e222c6c6e08`.

The source changes in [dmgez-profile.patch](dmgez-profile.patch) are relative to that exact archive. This profile keeps the upstream DMG writer and the signature primitives it uses. It omits the command-line frontend, key import, certificate-based signing and secure timestamps, bundle signing, notarization and stapling. The dependency graph excludes the associated HTTP and archive tools. Upstream tests and key fixtures are omitted from the snapshot. The root dmgez project qualifies the signer against its own staged-append checks and native Apple verification.

To compare the checked-in source with the published archive and the patch, download the [crate archive](https://static.crates.io/crates/apple-codesign/apple-codesign-0.29.0.crate) and run the verifier from this directory.

```sh
python3 verify-source.py path/to/apple-codesign-0.29.0.crate
```

The covered source remains under [MPL-2.0](LICENSE). Distributions containing this package should provide recipients access to the exact covered source used, including this patch, and retain the applicable license notices.
