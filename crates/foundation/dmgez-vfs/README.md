# Logical disk and partition access

`Disk::discover` classifies an immutable byte source independently of the UDIF block-map table layout. Pass a decoded `dmgez_udif::reader::Image` or a `Raw` source. `Layout` reports the scheme, logical block size, and map entries, or bounded HFS+ or HFSX volume geometry for a partitionless source. Unknown input has no inferred volume ID.

`Disk::select_volume` checks an explicit `VolumeId` for an eligible data extent or resolves the sole usable extent when the selector is omitted. It reports typed errors for zero or multiple eligible volumes. Map metadata, free space, and extended containers are excluded from automatic selection.

`Disk::window` opens the complete range for an explicit selector. `Disk::data_window` opens the usable data extent, including the APM data subrange when present. A window rechecks its bounds for every read. Subwindows preserve the parent bounds and absolute logical offsets. IDs refer to entries in the opened view.

`Window::write_raw_to` streams an opened extent into a caller-owned sink with a caller-sized buffer and cancellation check between chunks. The operation checks source identity at EOF before flushing. A failure can leave a prefix in the sink. Callers publishing a destination path own its staging and cleanup.

## Read-only filesystem views

`volume::VolumeView` takes an engine adapter opened on the selected extent. The partition selector identifies the extent, and each opened view issues its own root and visible node handles. Reopening the same extent creates a new handle namespace. Lookup accepts one component at a time using the engine's name policy and returns the stored name. A directory scan returns bounded pages in engine order. The engine controls its separate scan and I/O budgets.

Effective stat reports object kind, mode, ownership, and any admitted hard-link group. A group identifies shared storage within the view while its aliases retain separate visible handles. Regular files expose logical data and stored resource forks through bounded, exact readers. The view preserves a present zero-length fork and reports an absent fork as `None`. Symbolic links expose their own target bytes through `read_link` and reject fork access and child lookup.

## Supported map profiles

- APM uses 512-byte sectors, a driver descriptor, and up to 128 map entries. Reported entries include map metadata and free space. The map validates record counts, descriptor and partition bounds, declared data extents, and overlaps.
- GPT uses revision 1, 92-byte headers, 128-byte entries, and 512- or 4096-byte logical blocks. Both copies must agree and pass CRC checks. The protective MBR, metadata regions, partition intervals, and unique partition GUIDs are validated. The 4096-byte case has synthetic fixture coverage.
- MBR uses 512-byte sectors and supports primary slots and one bounded extended chain. Logical partitions start at slot five. The chain rejects cycles, out-of-container links, and logical overlaps with boot records and other data extents.

Hybrid GPT/MBR and overlapping maps return typed conflicts. Unsupported profiles include APM without a driver descriptor, alternate APM block sizes, optical dual-stride APM maps, alternate GPT entry widths, and multiple extended MBR containers. Inconsistent redundant GPT copies return corruption errors. Filesystem interpretation and verification belong to the independent filesystem engines.

The read adapters reuse the upstream parser APIs. Local guards bound `part-tables` input records and permit its 512-byte APM profile. Its EBR traversal permits partial success, so the VFS checks EBR chains and calls the library's fixed-sector parser per record. The VFS bounds GPT entry count and rejects entry widths outside the `gpt` parser's assertion before invoking it. The adapters pass bounded metadata copies to the decoders and preserve positional I/O failures from the source. [part-tables 0.1.0](https://docs.rs/part-tables/0.1.0/part_tables/) · [gpt 4.1.0](https://docs.rs/gpt/4.1.0/gpt/) · [Apple GPT format notes](https://developer.apple.com/library/archive/technotes/tn2166/_index.html) · [Apple HFS volume header](https://developer.apple.com/library/archive/technotes/tn/tn1150.html)

## Evidence

The integration tests cover the partition portion of D-READ-01-T01. Apple `hdiutil imageinfo` on the generated raw APM, GPT, MBR, and HFS+ fixtures reported the test ranges, and `hdiutil verify -quiet` verified all five compressed fixtures. A separate `hdiutil convert -format UDTO` extraction supplies the known length and checksum of the exported APM partition. The repository’s optical fixture records the unsupported dual-stride APM case. Focused tests exercise conflicting and damaged maps, malformed APM bounds, EBR cycles, probe budgets, ambiguous selection, nested range escapes, cancellation, and source and sink failures. The partitionless fixture covers the HFS+ geometry probe. Fixture provenance and digests are in [tests/fixtures/native-maps.json](tests/fixtures/native-maps.json).

The [volume tests](tests/volume.rs) cover the C5 view contract for D-READ-03 using a mutable backend. An isolated integration probe used `hfsplus-reader` at `5c872560d0e0816e2b34b39f3b44e516fc8eec4f` and its native chained-hard-link fixture inside UDZO. It exercised bounded root listing and lookup, effective metadata for three aliases, exact data and resource reads, and an empty resource fork. A separate image built by `hfsplus-builder` at the same revision exercised a non-UTF-8 symbolic-link target. The engine adapter and integration probe ran outside the workspace. Frontend commands, engine registration, and extended attribute reads are additional C5 work.

The crate-local Kani harness covers the range translation portion of D-V09. It checks four nested windows against widened arithmetic over full-width unsigned input. Other D-V09 proof and verification obligations remain tracked in the repository index. [Verification](verification/README.md)
