# Filesystem volume read compatibility

Run the development adapter with Python 3, the dmgez toolchain, and a populated Cargo cache. The dmgez checkout must include the extraction preflight introduced by `42e24a7`. The runner builds a temporary Cargo project from the chosen dependency paths, verifies each native fixture's SHA-256 digest, and runs locked, offline tests.

```sh
python3 tests/compatibility/volume-read/run.py --dmgez . --hfsplus /path/to/hfsplus
python3 tests/compatibility/volume-read/run.py --dmgez . --hfsplus-packages /path/to/unpacked
python3 tests/compatibility/volume-read/run.py --dmgez . --apfs /path/to/apfs
python3 tests/compatibility/volume-read/run.py --dmgez . --apfs-packages /path/to/unpacked
```

The package mode uses unpacked `hfsplus-core`, `hfsplus-format`, `hfsplus-reader`, and `hfsplus-builder` archives at version 0.1.0. Use a set that includes clean-journal support and the [pinned test fixtures](hfsplus/fixtures.json). Cargo path overrides apply inside the temporary project. The adapter stays outside the application workspace until the filesystem dependency is ready for production registration.

## HFS+

The adapter opens a bounded data window on the selected extent, then lends a session-bound view with HFS+ catalog and fork operations. HFS+ interprets UTF-8 component lookups and exact stored UTF-16 names. Directory pages preserve catalog order and the engine's scan limits. The adapter resolves regular-file aliases to their effective storage for metadata and fork reads. HFS+ stores a resource descriptor for every regular file, so an empty descriptor gives a zero-length resource reader. Logical data reads preserve typed errors from the HFS+ reader. This recipe uses the reader profile with the decmpfs decoder disabled.

The symbolic-link profile admits canonical HFS+ records with the BSD symlink type and matching Finder type and creator. It bounds target reads to 1,024 bytes or the configured lower limit and returns exact nonempty, NUL-free bytes. The view exposes these as link targets and keeps traversal within the selected volume.

Four tests passed against HFS+ revision `2571ebb3d47c0ebc056dc5f28fe7325eef61d0d0` and unpacked 0.1.0 archives generated from that revision, using dmgez revision `c7ea7a9e07d1e0ac7696483b34a023926f54ca60`. The tests use decoded UDZO over partitionless HFS+ volumes. They check the effective identity, metadata, and exact forks of three visible aliases, private hard-link catalog exclusion, and the exact file bytes and visible catalog on a clean journaled volume. Clearing the clean flag in a fixture copy must refuse the session before the callback. A separate constructed fixture checks exact non-UTF-8 symlink bytes. The reader package includes the independent macOS and 7-Zip evidence in `tests/fixtures/native-hardlink-resolution.json` and `tests/fixtures/native-journal.json`.

The fixture bytes cover selected recorded ranges. The tests reconstruct the disk with those bytes at their captured offsets and zeroes elsewhere. This evidence covers the D-READ-03 filesystem adapter operations exercised by the tests. The D-READ-03 large-image I/O budget and frontend workflows require separate qualification.

## APFS

The adapter opens the selected APFS partition, then a volume and checkpoint through the APFS reader. It uses exact stored UTF-8 names. It keeps each hard-link entry distinct from its shared inode and lends data and resource streams independently. Symbolic links expose their exact target bytes.

The tests passed against dmgez `1edf143` and APFS `a210ad5`. They stream a synthetic MBR and [pinned blocks](apfs/fixtures.json) from native APFS fixtures through UDZO. They exercise directory page recovery, separate hard-link and storage identities, both forks, absent and empty streams, foreign handle refusal, and an inline symlink. The fixture source refuses reads outside the captured APFS blocks.

The adapter also passed with unpacked `apfs-core`, `apfs-format`, `apfs-container`, and `apfs-reader` 0.1.0 archives from APFS `dd2e61a`. The runner verifies the pinned native blocks from the selected reader package before compiling.

## Extraction name preflight

Both recipes call `dmgez_operations::extract::preflight_names` on the real filesystem adapter. The HFS+ fixture constructed with a non-UTF-8 relative symlink target yields a portable name plan containing the link object and its source handle. The native APFS hard-link fixture preserves `Écho.bin` in the exact UTF-8 plan. The portable profile rejects that name with a typed encoding error. The APFS payload fixture provides targeted symlink lookup evidence. A full name scan of that fixture reaches uncaptured catalog block 408.

Select `--hfsx-fixture` for the native case-sensitive HFSX test. It requires HFS+ revision `eca5c48c6df694246b810a380f1d6b18ea11f4f7` or a compatible reader and the [pinned `native-hfsx.bin` capture](hfsplus/fixtures-hfsx.json). The reader package records the `fsck_hfs`, read-only mount and 7-Zip oracles in `tests/fixtures/native-hfsx-case-sensitive.json`. The base commands also work with the earlier `2571ebb` package set.

```sh
python3 tests/compatibility/volume-read/run.py --dmgez . --hfsplus /path/to/hfsplus --hfsx-fixture
python3 tests/compatibility/volume-read/run.py --dmgez . --hfsplus-packages /path/to/unpacked --hfsx-fixture
```

The portable profile rejects the distinct HFSX directories `Cases` and `cases` even when they arrive on separate pages. The bytewise UTF-8 profile preserves both and keeps `Case.txt` and `case.txt` as distinct children of `Cases`. These checks exercise name admission and source topology for D-READ-05.
