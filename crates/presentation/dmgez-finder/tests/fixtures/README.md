# Finder fixture provenance

`finder-macos.ds_store` is the 6148-byte `Test_DS_Store` fixture from the pinned `dmgbuild/ds_store` revision. The redistributable fixture contains three synthetic item names and was identified as Finder-authored by the source projects. The capture build of macOS was not recorded. The fixture matches the native sample packaged with `ds_parser` 0.4.0 byte-for-byte. [Upstream fixture](https://github.com/dmgbuild/ds_store/blob/d93392c32b92033789e8e7590f09812f7289873b/tests/Test_DS_Store) · [MIT notice](LICENSE-ds_store)

Its SHA-256 is `ee4885249f13d852331b0f88d1e42b5586b7424dc4acf11453da3651284ab4dd`. The fixture contains a single logical 4096-byte leaf with a 128-byte allocation. The icon centers are `bam` at (104, 116), `bar` at (256, 235), and `baz` at (454, 124).

## List view

`finder-list-14.7.2.ds_store` was written by Finder on macOS 14.7.2 build 23H311 on Apple Silicon. A disposable 16 MiB HFS+ image named Finder List Test contained one synthetic directory and one synthetic text file. Its volume root was opened in Finder, switched to list view and sorted by Size. The store was copied after Finder persisted it and the test image was detached. The captured `lsvp` stores keyed columns and the captured `lsvC` stores ordered columns, including the `dateAdded` column's 181-point width.

The original fixture is 10244 bytes. Its SHA-256 is `9784ad416019a13707bee4ba82d6ea1285f371900b597fc03365e633da9fde1d`. It contains no third-party assets or user files.
