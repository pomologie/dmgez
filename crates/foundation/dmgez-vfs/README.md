# Logical disk and partition access

`Disk::discover` classifies an immutable byte source independently of the UDIF block-map table layout. Pass a decoded `dmgez_udif::reader::Image` or a `Raw` source. `Layout` reports the scheme, logical block size, and map entries, or bounded HFS+ or HFSX volume geometry for a partitionless source. Unknown input has no inferred volume ID.

`Disk::window` selects an explicit `VolumeId` and grants access to the corresponding byte range. `Disk::data_window` selects the usable data extent, including the APM data subrange when present. A window rechecks its bounds for every read. Subwindows preserve the parent bounds. IDs refer to entries in the opened view.

## Supported map profiles

- APM uses 512-byte sectors, a driver descriptor, and up to 128 map entries. Reported entries include map metadata and free space. The map validates record counts, descriptor and partition bounds, declared data extents, and overlaps.
- GPT uses revision 1, 92-byte headers, 128-byte entries, and 512- or 4096-byte logical blocks. Both copies must agree and pass CRC checks. The protective MBR, metadata regions, partition intervals, and unique partition GUIDs are validated. The 4096-byte case has synthetic fixture coverage.
- MBR uses 512-byte sectors and supports primary slots and one bounded extended chain. Logical partitions start at slot five. The chain rejects cycles, out-of-container links, and logical overlaps with boot records and other data extents.

Hybrid GPT/MBR and overlapping maps return typed conflicts. Unsupported profiles include APM without a driver descriptor, alternate APM block sizes, optical dual-stride APM maps, alternate GPT entry widths, and multiple extended MBR containers. Inconsistent redundant GPT copies return corruption errors. Filesystem interpretation and verification belong to the independent filesystem engines.

The read adapters reuse the upstream parser APIs. Local guards bound `part-tables` input records and permit its 512-byte APM profile. Its EBR traversal permits partial success, so the VFS checks EBR chains and calls the library's fixed-sector parser per record. The VFS bounds GPT entry count and rejects entry widths outside the `gpt` parser's assertion before invoking it. The adapters pass bounded metadata copies to the decoders and preserve positional I/O failures from the source. [part-tables 0.1.0](https://docs.rs/part-tables/0.1.0/part_tables/) · [gpt 4.1.0](https://docs.rs/gpt/4.1.0/gpt/) · [Apple GPT format notes](https://developer.apple.com/library/archive/technotes/tn2166/_index.html) · [Apple HFS volume header](https://developer.apple.com/library/archive/technotes/tn/tn1150.html)

## Evidence

The integration tests cover the partition portion of D-READ-01-T01. Apple `hdiutil imageinfo` on the generated raw APM, GPT, MBR, and HFS+ fixtures reported the test ranges, and `hdiutil verify -quiet` verified all five compressed fixtures. The repository’s optical fixture records the unsupported dual-stride APM case. Negative tests exercise conflicting and damaged maps, malformed APM bounds, EBR cycles, probe budgets, and nested range escapes. The partitionless fixture covers the HFS+ geometry probe. Fixture provenance and digests are in [tests/fixtures/native-maps.json](tests/fixtures/native-maps.json).

The crate-local Kani harness covers the range translation portion of D-V09. It checks four nested windows against widened arithmetic over full-width unsigned input. Other D-V09 proof and verification obligations remain tracked in the repository index. [Verification](verification/README.md)
