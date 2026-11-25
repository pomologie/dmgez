# dmgez

A pure Rust library and CLI for creating, reading, and manipulating macOS disk images.

The CLI wraps an existing sector-aligned raw image in single-segment UDRO or UDZO. Enable `lzfse` for ULFO or `lzma` for ULMO. The optional `inspect` feature enables metadata reports, run-space analysis, container verification, and raw export.

```sh
cargo run --locked -- wrap-raw volume.raw -o volume.dmg
cargo run --locked -- wrap-raw volume.raw --format udro --serial -o uncompressed.dmg
cargo run --locked --features lzfse -- wrap-raw volume.raw --format ulfo -o compressed.dmg
cargo run --locked --features lzma -- wrap-raw volume.raw --format ulmo -o compressed.dmg
cargo run --locked --features inspect -- inspect volume.dmg
cargo run --locked --features inspect -- inspect volume.dmg --json --run-limit 32
cargo run --locked --features inspect -- inspect volume.dmg --partitions --json
cargo run --locked --features inspect -- analyze volume.dmg --json
cargo run --locked --features inspect -- verify volume.dmg
cargo run --locked --features inspect -- verify volume.dmg --quick --json
cargo run --locked --features inspect -- unwrap-raw volume.dmg -o exported.raw
cargo run --locked --features inspect -- unwrap-raw volume.dmg --partition 2 -o partition.raw
cargo run --locked --no-default-features --features convert -- convert volume.dmg --format udro -o uncompressed.dmg
cargo run --locked --no-default-features --features convert -- optimize volume.dmg -o optimized.dmg
cargo run --locked --no-default-features --features diff -- diff volume.dmg uncompressed.dmg --json
cargo run --locked --no-default-features --features lint -- lint project.toml --profile layout --folder /Docs --json
cargo run --locked --no-default-features --features installer-lint -- lint project.toml --profile installer --json
cargo run --locked --no-default-features --features signing-native -- sign verify installer.dmg --json
cargo run --locked --no-default-features --features appdmg-import -- import-appdmg settings.json -o project.toml --image-output installer.dmg
```

The CLI stages outputs in the destination directory and refuses to replace existing files. `unwrap-raw --partition` takes a one-based slot reported by `inspect --partitions` and exports that slot's complete logical extent. The default writer uses bounded parallel compression. A serial-only CLI can be built with `--no-default-features --features build`.

The optional `convert` command rebuilds qualified unsigned UDRO and UDZO containers. It verifies the source and compares the staged output's decoded data and admitted metadata before publication. It refuses inputs carrying opaque resources, signatures, or unqualified block maps.

The same feature enables `optimize`. It samples aligned windows within `--max-sample-chunks` and `--max-sample-bytes`, chooses the smaller measured UDRO or UDZO sample result, and publishes a verified conversion. It reports sampled offsets, wrapped sample sizes, and observed encoder times. Those measurements apply to the sampled windows.

The optional `diff` command compares decoded logical disk bytes by default. Use `--mode physical` to compare stored files or `--max-bytes` to change the comparison budget. Exit codes are 0 for equality, 1 for a difference, and 2 for an error or an exhausted budget. Reports identify the address space and length of the equal prefix. Filesystem semantics and integrity verification are separate checks.

Compose `diff` with `hfsplus-read` or `apfs-read` to compare filesystem contents and supported metadata under an explicit policy.

```sh
python3 tools/compose_filesystem_cli.py --apfs ../apfs --no-default-features --features diff
target/apfs-cli/debug/dmgez diff left.dmg right.dmg --mode semantic --filesystem apfs --json
```

[Semantic diff policy and selectors](docs/features/RELEASE.md#compare-filesystem-contents)

Use `--paths native` to compare path components using compatible on-disk name rules. This covers HFS+ and HFSX catalog comparators, bytewise APFS, and the Unicode 9 name profiles for insensitive APFS catalogs.

Build with `diff-hash` to enable `--contents blake3` or `--contents blake3-exact` and `--infer-renames` in semantic mode. Inferred pairs are listed alongside the added and removed paths.

Add `--xattrs` in semantic mode to compare supported stored extended attributes. The report identifies changed names without exposing their values.

Add `--finder-info` to compare per-object FinderInfo on common paths. It uses the selected filesystem reader and can run alongside `--xattrs` and `--finder-layout`.

Build with `finder-diff` and add `--finder-layout` in semantic mode to compare decoded layout records in matching `.DS_Store` files. Reports identify the affected records. Combine it with `--xattrs` to compare both policies in one run.

```sh
python3 tools/compose_filesystem_cli.py --apfs ../apfs --no-default-features --features finder-diff
target/apfs-cli/debug/dmgez diff left.dmg right.dmg --mode semantic --filesystem apfs --finder-layout --json
```

The optional `lint` command checks configured Finder icon geometry in one declared project folder, starting at `/` unless `--folder` selects another. It reads the TOML manifest only. Use `--suppress RULE=REASON` to keep a finding in the report without failing on it. Exit codes are 0 when there are no unsuppressed warnings, 1 when warnings remain, and 2 when the requested layout cannot be evaluated.

With `installer-lint`, `lint project.toml --profile installer --json` inspects the resolved source tree for the Applications shortcut, external symlink targets, referenced Finder backgrounds, and app bundle executables.

On macOS, `signing-native` enables `sign inspect` and `sign verify` for app bundles and UDIF images. The report distinguishes readable signatures, successful signature verification, unsigned artifacts, and signature failures. [Release checks](docs/features/RELEASE.md#inspect-and-verify-existing-signatures)

`sign dmg` uses an explicit Developer ID Application certificate in Keychain to sign a private copy of an unsigned image, then verifies and publishes it to a new path. [Stage and sign a disk image](docs/features/RELEASE.md#stage-and-sign-a-disk-image)

`sign app` signs a private bundle copy from the inside out and verifies the finished app before publishing to a new path. Use `--entitlements CODE_PATH=PLIST` to supply the distribution entitlements for a particular code item. [Stage and sign an application](docs/features/RELEASE.md#stage-and-sign-an-application)

With `notarization-native` on macOS, `notarize` submits a signed DMG with a private local record. `notarize status` resumes by Apple's submission ID. [Submit and resume notarization](docs/features/RELEASE.md#submit-and-resume-notarization)

`staple` matches a DMG to the accepted record's SHA-256, validates a ticket on a private copy, and publishes to a new path. [Staple a notarized disk image](docs/features/RELEASE.md#staple-a-notarized-disk-image)

`import-appdmg` translates documented appdmg JSON settings into a new version-one TOML manifest. Relative asset paths keep their original settings directory. A relative `--image-output` is interpreted from the new manifest's directory. The command reports unmapped behavior and refuses to replace an existing manifest.

## Build from a TOML project

While the versioned HFS+ dependencies are unpublished, compose the standalone CLI locally on macOS or Linux using a sibling checkout or unpacked 0.1.0 packages. The compose tool uses a temporary Cargo workspace and the local Cargo cache. The resulting binary contains the builder and runs directly.

```sh
python3 tools/compose_hfsplus_cli.py --hfsplus ../hfsplus
target/hfsplus-cli/debug/dmgez build project.toml --dry-run
target/hfsplus-cli/debug/dmgez build project.toml
```

Use `--hfsplus-packages PATH` for unpacked packages, or `--no-default-features --features build` for a serial build with Finder support. The compose tool's `--test` mode runs the build command's integration tests. Use `dmgez capabilities` to check whether the binary includes project building. The [compatibility runner](tests/compatibility/project-build/README.md) exercises the same build service and command implementation with the native fixtures.

To check reachable HFS+ and HFSX structure and read stored payloads, compose the CLI with `hfsplus-verify`.

```sh
python3 tools/compose_hfsplus_cli.py --hfsplus ../hfsplus --no-default-features --features hfsplus-verify
target/hfsplus-cli/debug/dmgez verify volume.dmg --full --json
```

A successful HFS+ check reports `reachable_stored_data` and the stored payload byte count. The result remains partial and exits nonzero while global filesystem and logical payload checks are outstanding. [Verification coverage](docs/features/INSPECTION.md#verify-is-separate)

Enable `hfsplus-read` to browse a UDIF image or an explicitly selected raw HFS+ or HFSX disk image. Add `hfsplus-verify` to the same composition to include structural verification.

```sh
python3 tools/compose_hfsplus_cli.py --hfsplus ../hfsplus --no-default-features --features hfsplus-read
target/hfsplus-cli/debug/dmgez ls volume.dmg --limit 32 --json
target/hfsplus-cli/debug/dmgez tree volume.dmg / --depth 2
target/hfsplus-cli/debug/dmgez stat volume.dmg /ReadMe.txt
target/hfsplus-cli/debug/dmgez cat volume.raw /ReadMe.txt --raw --fork data
```

Enable `hfsplus-decmpfs` for the qualified zlib-compressed HFS+ file profiles. Enable `hfsplus-extract` to copy an explicit fork or directory's regular-file data to a new host path on macOS or Linux.

```sh
python3 tools/compose_hfsplus_cli.py --hfsplus ../hfsplus --no-default-features --features hfsplus-extract
target/hfsplus-cli/debug/dmgez extract volume.dmg /ReadMe.txt --fork data --output ./ReadMe.txt
target/hfsplus-cli/debug/dmgez extract volume.dmg /Docs --data-only --output ./Docs --dry-run --json
```

## Browse APFS images

The optional APFS reader opens raw and UDIF disk images. Compose it on macOS or Linux from the sibling APFS checkout or unpacked 0.1.0 packages.

```sh
python3 tools/compose_filesystem_cli.py --apfs ../apfs --no-default-features
target/apfs-cli/debug/dmgez volumes image.dmg --json
target/apfs-cli/debug/dmgez ls image.dmg --filesystem apfs --volume 1026 --limit 32
target/apfs-cli/debug/dmgez cat image.raw /ReadMe.txt --raw --filesystem apfs
```

`--apfs-packages PATH` selects the archives. Use `--partition` to select a disk partition and `--volume` to select an APFS volume by UUID or decimal object ID. Refer to `volumes` for those IDs. Add `--xid` to open an exact retained checkpoint. [APFS reader scope](docs/features/INSPECTION.md#browse-apfs)

The same build checks reachable APFS metadata and reads logical file and attribute data within a budget. It reports `reachable_data` and exits with a partial verification result.

```sh
target/apfs-cli/debug/dmgez verify image.dmg --full --filesystem apfs --volume 1026 --json
```

[APFS verification scope](docs/features/INSPECTION.md#verify-apfs)

Compose `apfs-extract` on macOS or Linux to copy a selected APFS fork or directory's regular-file data to a new host path.

```sh
python3 tools/compose_filesystem_cli.py --apfs ../apfs --no-default-features --features apfs-extract
target/apfs-cli/debug/dmgez extract image.dmg /ReadMe.txt --filesystem apfs --fork data --output ./ReadMe.txt
target/apfs-cli/debug/dmgez extract image.dmg /Docs --filesystem apfs --data-only --output ./Docs --dry-run --json
```

[APFS extraction scope](docs/features/INSPECTION.md#extract-apfs)

## Desktop app

The [native desktop app](apps/dmgez/README.md) opens TOML projects for layout editing and disk images for container and partition inspection. Launch it with `cargo run --locked -p dmgez-gui`.

See the [architecture](ARCHITECTURE.md), [roadmap](ROADMAP.md), and [UDIF evidence](docs/reference/UDIF_EVIDENCE.md) for the implemented profile and its verification limits.
