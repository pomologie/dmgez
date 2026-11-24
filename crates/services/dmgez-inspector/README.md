# dmgez-inspector

`inspect` summarizes an admitted UDIF container without reading its payload. The optional run window returns bounded logical and physical ranges, original unknown run codes, and decoder availability. Block-map counts describe UDIF tables. Partition discovery is handled by the virtual filesystem service.

`verify` accepts an immutable UDIF source and reports the requested scope. Quick mode admits the metadata and ranges. Container mode decodes the stored runs and checks every declared checksum domain. Missing declarations remain `not_present`. Full mode reports partial coverage when the container passes and filesystem checks are unavailable. Security coverage stays `not_checked` in this profile.

Reports implement `Serialize` and include `schema_version`, `operation`, capabilities and per-layer coverage. Byte lengths and offsets serialize as decimal strings, and segment IDs serialize as fixed-width hexadecimal. Diagnostics record the first failure with a C6 category and numeric detail where the reader provides it. The local causal error is accessible through `VerificationReport::cause()` and stays out of the serialized payload. On an interrupted container scan the lower-level reader returns only the first error, so the per-domain fields retain declaration statuses.

The tests exercise the container portions of D-READ-03-T01 and D-READ-04-T01/T02 with native Apple fixtures, an instrumented metadata-only source, and synthetic checksum and run-code mutations. Filesystem verification, signature checks, and the full acceptance cases require separate evidence.
