# Filesystem volume read compatibility

Run the development adapter with Python 3, the dmgez toolchain, and a populated Cargo cache. The runner builds a temporary Cargo project from the chosen dependency paths, verifies each native fixture's SHA-256 digest, and runs locked, offline tests.

```sh
python3 tests/compatibility/volume-read/run.py --dmgez . --hfsplus /path/to/hfsplus
python3 tests/compatibility/volume-read/run.py --dmgez . --hfsplus-packages /path/to/unpacked
```

The package mode uses unpacked `hfsplus-core`, `hfsplus-format`, `hfsplus-reader`, and `hfsplus-builder` archives at version 0.1.0. Use a set that includes clean-journal support and the [pinned test fixtures](hfsplus/fixtures.json). Cargo path overrides apply inside the temporary project. The adapter stays outside the application workspace until the filesystem dependency is ready for production registration.

## HFS+

The adapter opens a bounded data window on the selected extent, then lends a session-bound view with HFS+ catalog and fork operations. HFS+ interprets UTF-8 component lookups and exact stored UTF-16 names. Directory pages preserve catalog order and the engine's scan limits. The adapter resolves regular-file aliases to their effective storage for metadata and fork reads. HFS+ stores a resource descriptor for every regular file, so an empty descriptor gives a zero-length resource reader. Logical data reads preserve typed errors from the HFS+ reader. This recipe uses the reader profile with the decmpfs decoder disabled.

The symbolic-link profile admits canonical HFS+ records with the BSD symlink type and matching Finder type and creator. It bounds target reads to 1,024 bytes or the configured lower limit and returns exact nonempty, NUL-free bytes. The view exposes these as link targets and keeps traversal within the selected volume.

Four tests passed against HFS+ revision `2571ebb3d47c0ebc056dc5f28fe7325eef61d0d0` and unpacked 0.1.0 archives generated from that revision, using dmgez revision `c7ea7a9e07d1e0ac7696483b34a023926f54ca60`. The tests use decoded UDZO over partitionless HFS+ volumes. They check the effective identity, metadata, and exact forks of three visible aliases, private hard-link catalog exclusion, and the exact file bytes and visible catalog on a clean journaled volume. Clearing the clean flag in a fixture copy must refuse the session before the callback. A separate constructed fixture checks exact non-UTF-8 symlink bytes. The reader package includes the independent macOS and 7-Zip evidence in `tests/fixtures/native-hardlink-resolution.json` and `tests/fixtures/native-journal.json`.

The fixture bytes cover selected recorded ranges. The tests reconstruct the disk with those bytes at their captured offsets and zeroes elsewhere. This evidence covers the D-READ-03 filesystem adapter operations exercised by the tests. The D-READ-03 large-image I/O budget and frontend workflows require separate qualification.
