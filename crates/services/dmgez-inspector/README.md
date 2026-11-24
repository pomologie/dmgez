# dmgez-inspector

`inspect` summarizes an admitted UDIF container without reading its payload. The optional run window returns bounded logical and physical ranges, original unknown run codes, and decoder availability. Block-map counts describe UDIF tables.

`inspect_disk` adds a bounded logical disk probe through `dmgez-vfs`. The optional `disk` object reports admitted APM, GPT or MBR entries, recognized partitionless HFS geometry, an unknown layout, or an unresolved layout with a redacted diagnostic. Partition ranges use decoded logical byte offsets. An unresolved layout retains the container summary and run page, including when a decoder is unavailable or the source cannot be read. All inspection coverage remains `not_checked`.

`verify` accepts an immutable UDIF source and reports the requested scope. Quick mode admits the metadata and ranges. Container mode decodes the stored runs and checks every declared checksum domain. Missing declarations remain `not_present`. Full mode reports partial coverage when the container passes and filesystem checks are unavailable. Security coverage stays `not_checked` in this profile.

Reports implement `Serialize` and include `schema_version`, `operation`, capabilities and per-layer coverage. Byte lengths and offsets serialize as decimal strings, and segment IDs serialize as fixed-width hexadecimal. Diagnostics record the first failure with a C6 category and numeric detail where the reader provides it. The local causal error is accessible through `VerificationReport::cause()` and stays out of the serialized payload. On an interrupted container scan the lower-level reader returns only the first error, so the per-domain fields retain declaration statuses.

The tests exercise the disk portion of D-READ-01-T01 and the container portions of D-READ-03-T01 and D-READ-04-T01/T02. They use [native Apple map fixtures](../../foundation/dmgez-vfs/tests/fixtures/native-maps.json), an instrumented metadata-only source, and synthetic map, source, checksum and run-code failures. Filesystem verification, signature checks, and the full acceptance cases require separate evidence.
