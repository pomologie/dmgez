# Filesystem volume read compatibility

Run the filesystem compatibility suite with Python 3, the dmgez toolchain, and a populated Cargo cache. The runner builds a temporary Cargo project from the chosen dependency paths, verifies each native fixture's SHA-256 digest, and runs locked, offline tests.

```sh
python3 tests/compatibility/volume-read/run.py --dmgez . --hfsplus /path/to/hfsplus
python3 tests/compatibility/volume-read/run.py --dmgez . --hfsplus-packages /path/to/unpacked
python3 tests/compatibility/volume-read/run.py --dmgez . --apfs /path/to/apfs
python3 tests/compatibility/volume-read/run.py --dmgez . --apfs-packages /path/to/unpacked
```

The package mode uses unpacked `hfsplus-pomologie-core`, `hfsplus-format`, `hfsplus-reader`, and `hfsplus-builder` archives at version 0.1.0 from the Pomologie HFS+ workspace. Cargo path overrides apply inside the temporary project. The HFS+ tests exercise the production `dmgez-hfsplus` adapter. The package must include the [pinned test fixtures](hfsplus/fixtures.json).

## HFS+

The adapter opens a bounded data window on the selected extent, then lends a session-bound view with HFS+ catalog and fork operations. HFS+ interprets UTF-8 component lookups and exact stored UTF-16 names. Directory pages preserve catalog order and the engine's scan limits. The session also limits the combined metadata and fork I/O, by default to one million positional requests and 4 GiB of decoded window bytes. The adapter resolves regular-file aliases to their effective storage for metadata and fork reads. HFS+ stores a resource descriptor for every regular file, so an empty descriptor gives a zero-length resource reader. Logical data reads preserve typed errors from the HFS+ reader. This recipe uses the reader profile with the decmpfs decoder disabled.

The symbolic-link profile admits canonical HFS+ records with the BSD symlink type and matching Finder type and creator. It bounds target reads to 1,024 bytes or the configured lower limit and returns exact nonempty, NUL-free bytes. The view exposes these as link targets and keeps traversal within the selected volume.

Four tests passed against HFS+ revision `2571ebb3d47c0ebc056dc5f28fe7325eef61d0d0` and unpacked 0.1.0 archives generated from that revision, using dmgez revision `c7ea7a9e07d1e0ac7696483b34a023926f54ca60`. Those archives predate the `hfsplus-pomologie-core` package rename. The tests use decoded UDZO over partitionless HFS+ volumes. They check the effective identity, metadata, and exact forks of three visible aliases, private hard-link catalog exclusion, and the exact file bytes and visible catalog on a clean journaled volume. Clearing the clean flag in a fixture copy must refuse the session before the callback. A separate constructed fixture checks exact non-UTF-8 symlink bytes. The reader package includes the independent macOS and 7-Zip evidence in `tests/fixtures/native-hardlink-resolution.json` and `tests/fixtures/native-journal.json`.

The fixture bytes cover selected recorded ranges. The tests reconstruct the disk with those bytes at their captured offsets and zeroes elsewhere. In an instrumented 16 GiB logical disk containing the recorded HFS+ volume, layout discovery, one directory page, and hard-link metadata used 55 reads totaling 195,118 bytes without reading a file payload block. The test checks both seven-byte fork requests at their recorded physical ranges and rejects budget overruns before dispatching the next header read. This covers the filesystem adapter portions of D-READ-03-T01 and D-READ-03-T02. [I/O witness](hfsplus/tests/browse_budget.rs)

The native catalog session lends the original volume and catalog for rebuild planning. It shares the browse path's I/O budgets and exposes stored fork readers without materializing their payloads. The HFS+ hard-link fixture confirms that opening both forks reads metadata only and that subsequent reads request the exact data and resource byte ranges. [Catalog session](../../../crates/adapters/dmgez-hfsplus/src/browse/session.rs) · [I/O witness](hfsplus/tests/browse_budget.rs)

## APFS

The tests exercise the production `dmgez-apfs` adapter. It opens a bounded disk extent, selects a checkpoint and volume through the APFS reader, and matches exact stored UTF-8 names. It keeps each hard-link entry distinct from its shared inode and lends data and resource streams independently. Symbolic links expose their exact target bytes.

The tests passed against dmgez `1edf143` and APFS `a210ad5`. They stream a synthetic MBR and [pinned blocks](apfs/fixtures.json) from native APFS fixtures through UDZO. They exercise directory page recovery, separate hard-link and storage identities, both forks, absent and empty streams, foreign handle refusal, and an inline symlink. The fixture source refuses reads outside the captured APFS blocks.

The adapter also passed with unpacked `apfs-core`, `apfs-format`, `apfs-container`, and `apfs-reader` 0.1.0 archives from APFS `dd2e61a`. The runner verifies the pinned native blocks from the selected reader package before compiling.

Run the two-volume oracle on macOS with a composed APFS CLI.

```sh
python3 -B tests/compatibility/volume-read/check_apfs_cli_multivolume.py target/apfs-cli/debug/dmgez
```

On macOS 14.7.2 (23H311, arm64), the script generated a disposable 1 GiB APFS container with two volumes without mounting either one. Apple checked the detached raw image and both volumes with `fsck_apfs -n`, which emitted a disk-name warning for the regular image path. The CLI matched Apple's volume UUIDs in both the raw image and Apple's UDZO conversion. Each UUID and object ID resolved to its selected root at XID 3, and browsing without a selector refused the ambiguous container. [Native output and image digests](apfs/native-cli-multivolume.json)

Pass `--verify` to also compare selected-volume verification. The native oracle runs `hdiutil verify` on the UDZO image, checks that both input types require a selector, and expects partial reachable coverage for each selected empty volume at the same checkpoint. It compares image digests before and after the checks. [Native verification reports](apfs/native-cli-verification.json)

Run the native extraction oracle with an `apfs-extract` build.

```sh
python3 -B tests/compatibility/volume-read/check_apfs_extraction.py --cli target/apfs-cli/debug/dmgez
```

The macOS oracle populates and detaches a disposable APFS image, then checks raw structure with `fsck_apfs -n` and Apple's UDZO conversion with `hdiutil verify`. A read-only mount checks the two hard-link aliases, fork bytes, mode, and markers. dmgez exports the selected clean paths, refuses marked provenance, and leaves the image bytes intact. [Native extraction results](apfs/native-cli-extraction.json)

Run the native semantic comparison oracle with a CLI composed with `diff` and `apfs-read`.

```sh
python3 -B tests/compatibility/volume-read/check_semantic_diff.py --cli target/apfs-cli/debug/dmgez
```

The oracle creates one filesystem and compares its raw, UDRO, and UDZO representations. It then edits the disposable source and confirms the test changes on read-only mounts. dmgez identifies separate fork, mode, symlink, hard-link, and path changes. [Native comparison results](apfs/native-semantic-diff.json)

Pass `--hash` with a CLI composed with `diff-hash` and `apfs-read` to check BLAKE3 comparison, exact confirmation, and inference of the native file move. The oracle checks a physical difference between UDRO and UDZO, semantic equality in both hash modes, and a candidate pair alongside the ordinary added and removed paths. [Native hash and rename results](apfs/native-semantic-hash-diff.json)

```sh
python3 tools/compose_filesystem_cli.py --apfs ../apfs --no-default-features --features diff-hash
python3 -B tests/compatibility/volume-read/check_semantic_diff.py --hash --cli target/apfs-cli/debug/dmgez
```

The native extended-attribute oracle uses both APFS and HFS+ readers. It verifies complete attribute values on read-only macOS mounts and checks name-only difference reports.

```sh
python3 tools/compose_filesystem_cli.py --apfs ../apfs --hfsplus ../hfsplus --no-default-features --features diff,hfsplus-read
python3 -B tests/compatibility/volume-read/check_xattr_diff.py --cli target/apfs-cli/debug/dmgez
```

[Native extended-attribute results](native-xattr-diff.json)

The native per-object FinderInfo check uses `diff` and both readers. Unpack the four APFS 0.1.0 archives built from `78f8586` and the HFS+ 0.1.0 archives built from `fb11b35` for the reader APIs used by current composition. The sibling source checkouts support the same recipe.

```sh
python3 tools/compose_filesystem_cli.py --apfs ../apfs --hfsplus ../hfsplus --no-default-features --features diff,hfsplus-read
python3 -B tests/compatibility/volume-read/check_finder_info_diff.py --cli target/apfs-cli/debug/dmgez
```

The oracle compares the CLI's findings with exact FinderInfo read through native `xattr` on read-only APFS and HFS+ mounts. It checks same-inode aliases, symbolic links, attribute presence, content and resource fork preservation, and unchanged image digests. Run the script with `--native-only` to exercise the fixture and independent checks. [Native FinderInfo results](native-finder-info-diff.json)

The native Finder layout oracle uses `finder-diff` and the APFS reader. Install `ds_store==1.3.4` and `mac_alias==2.2.3` in the Python environment used for the oracle.

```sh
python3 tools/compose_filesystem_cli.py --apfs ../apfs --no-default-features --features finder-diff
python3 -B tests/compatibility/volume-read/check_finder_diff.py --cli target/apfs-cli/debug/dmgez
```

The oracle verifies and mounts three disposable UDZO images read-only. Its independent decoder checks an icon position and a column width in both list-view representations. It also checks that the default policy detects unused store bytes and that the Finder policy finds the same decoded records. [Native Finder layout results](apfs/native-finder-diff.json)

## Native path comparison

Compose with `diff` and `hfsplus-read` against HFS+ sources with `NameCollation::key`, or archives built from `fb11b35`. The macOS check verifies native path resolution and exact file bytes on disposable HFS+ and binary HFSX images mounted read-only.

```sh
python3 tools/compose_filesystem_cli.py --hfsplus ../hfsplus --no-default-features --features diff,hfsplus-read
python3 -B tests/compatibility/volume-read/check_native_paths_diff.py --cli target/hfsplus-cli/debug/dmgez
```

The oracle checks both stored spellings, exact mode, the native profile's path pairings, and unchanged image digests. Use `--native-only` to run the independent image checks. [Native path reports](native-paths-diff.json)

For APFS, compose against `78f8586` or compatible sources with `VolumeView::name_profile`. The macOS oracle checks both Unicode 9 profiles, verifies that two distinct file inodes share a real 22-bit directory hash, and checks the explicit rejection of a filename containing a scalar introduced after Unicode 9. Exact stored-name comparison remains available for that image.

```sh
python3 tools/compose_filesystem_cli.py --apfs ../apfs --no-default-features --features diff
python3 -B tests/compatibility/volume-read/check_apfs_native_paths_diff.py --cli target/apfs-cli/debug/dmgez
```

The `--native-only` flag runs the read-only mount and image checks without the CLI. [Native APFS name reports](apfs/native-unicode-paths-diff.json)

## Extraction name preflight

Both recipes call `dmgez_operations::extract::preflight_names` on the real filesystem adapter. The HFS+ fixture constructed with a non-UTF-8 relative symlink target yields a portable name plan containing the link object and its source handle. The native APFS hard-link fixture preserves `Écho.bin` in the exact UTF-8 plan. The portable profile rejects that name with a typed encoding error. The APFS payload fixture provides targeted symlink lookup evidence. A full name scan of that fixture reaches uncaptured catalog block 408.

Select `--hfsx-fixture` for the native case-sensitive HFSX test. The fixture was introduced in HFS+ revision `eca5c48c6df694246b810a380f1d6b18ea11f4f7`. Use a workspace containing the renamed core package, a compatible reader and the [pinned `native-hfsx.bin` capture](hfsplus/fixtures-hfsx.json). The reader package records the `fsck_hfs`, read-only mount and 7-Zip oracles in `tests/fixtures/native-hfsx-case-sensitive.json`.

```sh
python3 tests/compatibility/volume-read/run.py --dmgez . --hfsplus /path/to/hfsplus --hfsx-fixture
python3 tests/compatibility/volume-read/run.py --dmgez . --hfsplus-packages /path/to/unpacked --hfsx-fixture
```

The portable profile rejects the distinct HFSX directories `Cases` and `cases` even when they arrive on separate pages. The bytewise UTF-8 profile preserves both and keeps `Case.txt` and `case.txt` as distinct children of `Cases`. These checks exercise name admission and source topology for D-READ-05.
