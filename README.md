# dmgez

A pure Rust library and CLI for creating, reading, and manipulating macOS disk images.

See [offline CLI archives](docs/features/OFFLINE_CLI.md) for packaging a composed build and installing the CLI from an unpacked release.

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

Build with `remote` to inspect an explicit HTTP or HTTPS image URL with `dmgez inspect --url URL --json`. The remote endpoint must support bounded byte ranges and a strong ETag. [Remote inspection](docs/features/ADVANCED.md#remote-reader)

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

The default CLI uses `apple-codesign` for `sign dmg --ad-hoc` on macOS, Linux, and Windows. Set `[signing]` in `dmg.toml` and run `dmgez build dmg.toml --release` to sign during a project release. Developer ID signing uses the macOS Keychain backend. [Signing configuration](docs/features/RELEASE.md#portable-signing-profile)

`sign app` signs a private bundle copy from the inside out and verifies the finished app before publishing to a new path. Use `--entitlements CODE_PATH=PLIST` to supply the distribution entitlements for a particular code item. [Stage and sign an application](docs/features/RELEASE.md#stage-and-sign-an-application)

With `notarization-native` on macOS, `notarize` submits a signed DMG with a private local record. `notarize status` resumes by Apple's submission ID. [Submit and resume notarization](docs/features/RELEASE.md#submit-and-resume-notarization)

`staple` matches a DMG to the accepted record's SHA-256, validates a ticket on a private copy, and publishes to a new path. [Staple a notarized disk image](docs/features/RELEASE.md#staple-a-notarized-disk-image)

Compose `installer-test` to check the finished image with `dmgez test installer.dmg --profile installer --json`. The report keeps container and app layout results separate from the requested signature, ticket, and native policy checks. [Finished installer test](docs/features/RELEASE.md#test-a-finished-installer)

`import-appdmg` translates documented appdmg JSON settings into a new version-one TOML manifest. Relative asset paths keep their original settings directory. A relative `--image-output` is interpreted from the new manifest's directory. The command reports unmapped behavior and refuses to replace an existing manifest.

## Build from a TOML project

The ordinary Cargo workspace builds the raw-image CLI. To enable `dmgez build`, compose the standalone CLI with the unpublished HFS+ packages on macOS, Linux, or Windows. Use Python 3.11 or newer and the Rust toolchain pinned in this checkout. Supply a sibling `hfsplus` checkout containing commit `d3daf6e70c2e46cbae6812f19eb310abf4006afb`.

Run from the dmgez checkout. Set the manifest and output paths for your project. The build resolves `[image].output` and asset paths relative to `dmg.toml`. Upload the verified output file from that path in your CI job.

```sh
git -C ../hfsplus checkout --detach d3daf6e70c2e46cbae6812f19eb310abf4006afb
python3 tools/compose_hfsplus_cli.py --hfsplus ../hfsplus --fetch --release --features inspect
target/hfsplus-cli/release/dmgez build path/to/dmg.toml --dry-run
target/hfsplus-cli/release/dmgez build path/to/dmg.toml
target/hfsplus-cli/release/dmgez verify path/to/installer.dmg --container
```

On Windows, place `dmg.toml` and its assets in a project directory on local NTFS, such as `$env:LOCALAPPDATA\Temp\dmgez-project`. Keep the output on the same local filesystem. The Windows adapter reports metadata it cannot observe. Strict builds refuse required metadata loss. [Windows source profile](docs/reference/WINDOWS_SOURCES.md)

```powershell
git -C ..\hfsplus checkout --detach d3daf6e70c2e46cbae6812f19eb310abf4006afb
py -3 tools\compose_hfsplus_cli.py --hfsplus ..\hfsplus --fetch --release --features inspect
$projectDirectory = Join-Path $env:LOCALAPPDATA 'Temp\dmgez-project'
$manifest = Join-Path $projectDirectory 'dmg.toml'
$image = Join-Path $projectDirectory 'installer.dmg'
.\target\hfsplus-cli\release\dmgez.exe build $manifest --dry-run
.\target\hfsplus-cli\release\dmgez.exe build $manifest
.\target\hfsplus-cli\release\dmgez.exe verify $image --container
```

`--fetch` primes Cargo's cache using the workspace lockfile. The composer then builds with the supplied HFS+ packages in a temporary workspace with frozen dependencies. Use `--hfsplus-packages PATH` for unpacked 0.1.0 packages, `--no-default-features --features build` for serial builds with Finder support, or `--test` to run the build integration tests. [Project build fixtures](tests/compatibility/project-build/README.md)

The workflow runs on manual dispatch from the default branch. It needs the pinned HFS+ commit available to GitHub Actions. For a private HFS+ checkout, configure `HFSPLUS_READ_TOKEN` with read-only access to that repository. The default GitHub token suffices for a public checkout. The workflow verifies and compares its own fixture artifacts. The NTFS path passed a Windows-target compilation check. Native Windows runtime qualification remains pending. [Cross-platform DMG project build workflow](.github/workflows/project-build.yml)

Compose `project-edit` to preview a typed TOML edit batch and rebuild to a verified unsigned image at a fresh path. [Edit and rebuild a project](docs/features/ADVANCED.md#edit-and-rebuild-a-project)

Compose `image-edit` to apply a typed batch of moves, removals and additions to an existing HFS+ disk image and publish a verified unsigned rebuild. [Edit an existing HFS+ image](docs/features/ADVANCED.md#edit-an-existing-hfs-image)

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

## Install an app without mounting

Compose `hfsplus-install` or `apfs-install` on macOS or Linux. Select one `.app` and an existing destination directory. Run with `--dry-run` to check the plan and destination.

```sh
python3 tools/compose_filesystem_cli.py --hfsplus ../hfsplus --no-default-features --features hfsplus-install
target/hfsplus-cli/debug/dmgez install installer.dmg --app /Example.app --destination ./output --json
```

[Supported app metadata and publication behavior](docs/features/MOUNT_INSTALL.md#virtual-install-and-external-copy)

## Desktop app

The [native desktop app](apps/dmgez-desktop/README.md) opens TOML projects for layout editing and disk images for container and partition inspection. Launch it with `cargo run --locked -p dmgez-gui`.

Run `dmgez open installer.dmg` when `dmgez-gui` is installed beside the CLI. Use `dmgez open dmg.toml --gui /path/to/dmgez-gui` to select another installation. On Windows, select the desktop `.exe`. The CLI returns once the desktop process starts and reports missing installations on stderr.

The [local stdio MCP server](apps/dmgez-mcp/README.md) exposes capabilities and validates TOML manifests inside an explicitly authorized project directory on macOS and Linux.

See the [architecture](docs/architecture/ARCHITECTURE.md), [roadmap](docs/architecture/ROADMAP.md), and [UDIF evidence](docs/reference/UDIF_EVIDENCE.md) for the implemented profile and its verification limits.
