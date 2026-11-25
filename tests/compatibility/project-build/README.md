# HFS+ build compatibility

Run this development exercise against adjacent source checkouts with an existing Cargo cache.

The runner links the product-owned `dmgez-hfsplus` service and copies the actual CLI build command into its isolated test package. Use the [local composition tool](../../../README.md#build-from-a-toml-project) to compile `dmgez build` directly into a standalone binary.

```sh
python3 run.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus
python3 run.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus --project /path/to/project.toml
python3 run.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus \
  --project /path/to/project.toml --define VERSION=2.0 \
  --set image.output=Example.dmg --dry-run
```

Use `--hfsplus-packages /path/to/unpacked` to select an unpacked set of versioned `hfsplus-builder`, `hfsplus-format`, and `hfsplus-pomologie-core` 0.1.0 archives from the Pomologie HFS+ workspace. The runner resolves paths before Cargo and confines path patches and source copies to a temporary Cargo project. The default profile includes parallel encoding and Finder store generation. Use `--no-default-features` for serial encoding without Finder store generation.

Use `--lzma` to enable ULMO writing and verification with either profile. Select it with `format = "ulmo"` in the TOML `[image]` section or with `--set image.format=ulmo`. Requests for ULMO without `--lzma` fail during preflight, including in dry-run.

```sh
python3 run.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus --lzma \
  --project /path/to/project.toml --set image.format=ulmo
python3 run.py --dmgez /path/to/dmgez --hfsplus-packages /path/to/unpacked \
  --no-default-features --lzma
```

`--define NAME=VALUE` supplies a literal project substitution. `--set section.field=value` overrides one allowlisted scalar with normal config type and semantic validation. The last value for a repeated name or key wins. Relative TOML paths, including paths built from substitutions, use the config file's directory. The `--project` path and relative path values supplied directly with `--set` use the caller's working directory.

`--dry-run` reports the exact planned virtual size, requested encoding, publication and release intent, source counts and metadata-loss count. The `finder` field reports the root default as `IconView`, `ListView`, or `NoStore`. The physical UDIF size is unknown until encoding. The planner checks source identity, capacity and catalog collisions during dry-run. It leaves the output path and its directory untouched. Progress and up to twenty metadata-loss details go to stderr, with a count of further losses.

The default publication policy preserves an existing destination. Authorize replacement with `--overwrite`, `--set build.overwrite=true`, or `[build] overwrite = true` in the project. Choose a single CLI form for this setting. Use replacement only in a trusted local directory where publishers cooperate with the advisory lock. The builder checks source and destination identity, syncs the completed stage, and atomically replaces the destination. The stdout build summary follows successful publication.

The adapter uses dmgez preflight, source discovery and identity checks, Finder store generation, and staged publication. It binds generated payloads after HFS+ assigns catalog IDs, selects capacity with the same HFS+ planner used for placement, and streams the resulting image through UDIF. The manifest and sealed plan carry borrowed regular-source identities. Each bounded file read acquires an identity-checked handle and releases it before proceeding.

Select the root list layout with `[finder] default_view = "list-view"` and put its settings under `[finder.list_view]`. A folder inherits the root default view and list settings. Local scalar settings and individual width or sort-direction overrides replace inherited values. A local `columns` array sets visibility and order for that folder. The [list project fixture](tests/fixtures/finder-list.toml) demonstrates a nested override. The serial build without Finder store generation rejects a layout request during preflight.

Run the native list check on macOS with `ds_store` 1.3.4 and `mac_alias` 2.2.3 available in the Python environment.

```sh
python3 check_finder_list.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus
```

The check runs the project in dry-run and build modes, verifies the UDIF with Apple tools, mounts it read-only and decodes both root and nested list stores. It checks exact view, sort, encoded column data and inherited settings in the keyed and ordered records, plus the independent icon anchor. It detaches its own test image.

On macOS 14.7.2 (23H311), the fixture produced a 1 MiB logical disk with two Finder assets and zero metadata losses. Apple verified the UDIF and both mounted stores matched the expected binary-plist types and values.

The background fixtures use synthetic 2×2 RGB PNGs and a 4×4 RGBA Retina PNG. The compatibility suite checks root and inherited aliases, nested image and color overrides, exact file data, malformed images, source mutation, and catalog identity. Its TIFF check reads both directories and compares density, pixel data, and alpha. Use `--include-ignored` in test mode to enable the independent Python alias checks. With `ds_store` 1.3.4, `mac_alias` 2.2.3, and Apple's Swift compiler available, run the native mount check on macOS.

```sh
python3 check_finder_background.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus
```

The native check verifies each image with Apple tools, runs the HFS+ filesystem checker, and mounts the image read-only. Core Foundation resolves each alias without opening UI or mounting another volume. The test checks the catalog IDs and exact static PNG or prebuilt TIFF bytes. ImageIO compares both TIFF representations to the source pixels, and AppKit confirms their logical sizes and selection at 1× and 2×. The prebuilt case uses native `tiffutil` to create its input. [Core Foundation resolution options](https://developer.apple.com/documentation/corefoundation/bookmark-data-resolution-options?changes=l_2&language=objc)

Use `--volume-icon` with a prebuilt ICNS, a set of PNG representations, or a per-item ICNS configured relative to the project.

```toml
[volume_icon]
source = "artwork/Example.icns"
```

```toml
[volume_icon.images]
"16" = "artwork/icon_16x16.png"
"16@2x" = "artwork/icon_16x16@2x.png"
"512" = "artwork/icon_512x512.png"
```

The adapter writes the ICNS to the root `.VolumeIcon.icns` file and sets the custom-icon bit on the root catalog record. Dry-run checks the asset and includes it in the planned capacity without staging an output. Each source uses the same identity and output-alias checks as copied content. The retained ICNS counts toward the metadata budget, and the validation and assembly workspace counts toward the build working budget. The [volume icon profile](../../../crates/presentation/dmgez-finder/VOLUME_ICON.md) describes the encodings and limitations.

The native PNG-set check uses Apple's Swift compiler and `iconutil`. It publishes a disposable project image, verifies and mounts it read-only, checks the root flag with `GetFileInfo`, and checks dimensions and pixels for all ten input slots.

```sh
python3 check_finder_volume_icon.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus
```

Set `custom_icon` on a regular file or a folder. The compatibility suite checks the resource map, catalog flags, exact ICNS bytes, the invisible folder helper, and output preservation when the project or icon source changes.

```toml
[[contents]]
source = "Document.txt"
destination = "/Document.txt"
custom_icon = "artwork/Document.icns"

[[contents]]
type = "directory"
destination = "/Documentation"
custom_icon = "artwork/Folder.icns"
```

On macOS, the first check builds and mounts a disposable image read-only and reads both icons through AppKit. The second creates and verifies ad-hoc signed fixtures and checks publication refusal.

```sh
python3 check_finder_item_icons.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus
python3 check_signed_item_icon.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus
```

Use `--badge-icons` to enable badged volume icons in the runner. A standalone composed CLI enables it with `--features badge-icons` during composition.

```toml
[finder]
background = { kind = "template", name = "builtin-arrow" }

[volume_icon]
badge = "artwork/Badge.png"
```

The independent template test checks both TIFF pages and DPI tags, verifies the catalog alias, and covers inheritance and reserved paths. The badge tests check all ten ICNS slots and their sampled pixels, enforce selection of the largest ICNS source representation, and reject source aliasing and changes before publication. The combined native check uses the same pinned Python packages as the background check and Apple's Swift compiler.

```sh
python3 check_finder_artwork.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus
```

The qualified source profile covers ordinary regular file permissions and ownership, timestamp conversion at HFS+ precision, inline attributes, Finder information, a buffered resource fork, and exact symlink target bytes. The Unicode profile gives included regular-file hard links distinct catalog entries that share one chain-free inode, with both forks and attributes captured once. Source checks cover every alias before publication. Shared metadata losses name the representative and its other image paths. The strict policy rejects unavailable required directory and symlink attributes, subsecond date loss, unsupported BSD flags and attribute encodings, and hard-link groups outside the supported regular-file profile. Report policy records those losses, and an explicit hard-link preservation opt-out emits independent files with omission diagnostics. Explicit timestamp omission and an explicit generated epoch suit deterministic synthetic fixtures.

## Windows NTFS composition

Run the standalone HFS+ composition tool with Python 3.11 or newer on local NTFS. Use the checkout or unpacked package form. Add `--test` on a native Windows machine to execute the project build tests, including the NTFS source and metadata fixture.

```powershell
py -3 tools\compose_hfsplus_cli.py --hfsplus ..\hfsplus --test
py -3 tools\compose_hfsplus_cli.py --hfsplus ..\hfsplus
target\hfsplus-cli\debug\dmgez.exe build dmg.toml --dry-run
target\hfsplus-cli\debug\dmgez.exe build dmg.toml
```

Windows copies regular file data from the [NTFS source adapter](../../../crates/services/dmgez-builder/WINDOWS_SOURCES.md). Requested attributes that the host cannot observe fail in strict metadata mode. `[metadata] policy = "report"` permits the build and reports omissions. The Windows builder publishes to a new output and refuses `--overwrite`. Finder custom item icons require host metadata conflict checks and are unavailable on this path. APFS and filesystem reader composition continue to require macOS or Linux.

Cross-target Clippy with warnings denied passed on Rust 1.99.1 for `x86_64-pc-windows-msvc`, composed with HFS+ revision `fb11b35`. The Windows-specific fixture covers dry-run, a copied Unicode-named source, strict metadata refusal, report-mode publication, and no-clobber behavior. Native Windows runtime evidence is pending.

## Native interoperability evidence

The smoke tests assert minimum free capacity, byte-exact regular-file, inline attribute and macOS resource-fork read-back, an insufficient shrink bound, and rejection of source changes before publication. Hard-link tests verify a shared inode and single attribute owner, distinct catalog IDs, independent equal-content files, byte-identical output after reordering declarations, and rejection when the nonrepresentative source path is replaced. Native smoke evidence additionally runs Apple's UDIF checksum verifier and UDTO export on a disposable generated image, checks exact file and symlink bytes with 7-Zip, and reads both Finder positions with the Python ds_store oracle.

The ULMO command test checks refusal before source discovery with the codec disabled. With it enabled, the test checks exact dry-run capacity, requires an LZMA2 run, verifies UDIF checksums, and compares the full decoded disk against the same project built as UDRO. On macOS 14.7.2 (23H311), `hdiutil verify` accepted both default and serial ULMO builds. UDTO exports matched the corresponding 1 MiB UDRO baselines byte for byte. Each profile produced the same ULMO bytes with sibling sources and unpacked HFS+ 0.1.0 packages from before the `hfsplus-pomologie-core` package rename.

A separate read-only mount on macOS 14.7.2 verified that both hard-link aliases had the same inode and link count of two. It also checked the independent file's inode, executable mode, and both aliases' exact data, resource fork, and inline attribute.
