# UDIF verification

## v0.1.0 acceptance

The locally exercised UDIF acceptance cases below bind to dmgez `1ace11a477123d40be39f2c062d67dec72fc3bfc` on macOS 14.7.2 (23H311) arm64 with Rust 1.99.1. Verification of the Windows runtime and the hosted cross-platform project workflow is tracked in the [release profile](../../../../docs/features/RELEASE.md#v010-release-profile).

| Cases | Evidence |
| --- | --- |
| D-UDIF-01-T01 to T03 | The native tiny UDRO descriptor matches the recorded field ledger. The reader fetches the 512-byte trailer and exact XML range. Invalid header and count fields, payload-only magic, wrapping XML, and XML intersecting the trailer refuse after at most one trailer read. All 512 sub-trailer lengths refuse without a read. The exact native trailer bytes also round-trip through apple-dmg 0.5.0. The earlier Kani run covers arbitrary full-width trailer ranges. |
| D-UDIF-02-T01 to T04 | Apple verifies generated zero-only, mixed, and control-run images against independent CRC expectations. The native zero and ignore pair shares a physical CRC of `cd44d198` and has distinct table CRCs of `2f96947b` and `cd44d198`. The checksum tests distinguish absent, unsupported and mismatched declarations, flip one bit in each declared domain independently, and reject invalid comment counts and missing terminators before payload reads. |
| D-UDIF-03-T01 to T03 | Fragmented sources and a nonseekable sink preserve exact bytes and contiguous write offsets. A short or long source yields a typed length error and the staged builder leaves no destination or temporary image. A short sink that stops progressing leaves an unfinished prefix. A separate two-chunk fixture exercises raw fallback and decodes to the original bytes with Apple, apple-dmg 0.5.0, and udif 0.4.0 for each output profile. |
| D-UDIF-04-T01 to T03 | Generated raw, zlib and zero runs match independent raw slices through unaligned, random and EOF reads. The reader refuses logical overlap, physical reuse, unknown runs, invalid zlib length or framing, and oversized declared runs. The instrumented production inflater requests one 513-byte output buffer for a 512-byte run and refuses a 64 KiB expansion. A positioned read of the final sparse byte also succeeds on a logical disk larger than a byte buffer. |

The all-feature UDIF suite passed 98 tests with 4,096 property cases and seed `20261126`. The read-only trailer suite passed five tests, the read/write reader limit suite passed four, and the write-only streaming suite passed four. The builder passed 13 execution tests. AddressSanitizer replayed 20,000 inputs from eight checked-in images with a 64 KiB input limit and produced no crash artifacts. The fresh composed CLI using HFS+ `a1b870f` reproduced all four signed and unsigned project digests with Apple and p7zip 17.05 checks. The replay used Apple verification and raw exports. The earlier native attachment and read-only filesystem evidence remains in the release profile.

To build the standalone reference adapter, copy [reference_readers.rs](reference_readers.rs) to `src/main.rs` in a disposable Cargo package outside this workspace. Pin `apple-dmg = "=0.5.0"` and `udif = "=0.4.0"`, then build it and pass its executable to [verify_udif.py](../../../../tests/compatibility/verify_udif.py) with `--reference-readers`. The two Rust reference readers exercise one-table raw and zlib cases. Apple tools check the explicit zero, ignore, and comment fixtures. 7-Zip independently checks the tiny, mixed, and zero-only images. The adapter has no production dependency role.

The normalized logs and the standalone oracle lockfile are retained locally under `target/verification-evidence/udif-v0.1.0/`.

| Artifact | SHA-256 |
| --- | --- |
| Rust checks | `ff27761511257c965dbb38fc8907a70a2a9c019ad3efcc089fb77d2bb6aea766` |
| Apple, p7zip and named reference-reader replay | `83489e657a7a1b0d59094cd93e39e51e17081cc2864695e657b6067a764e8b06` |
| AddressSanitizer run | `016cd9efd859bd989982e96166b80e9e0eda7bc7a0f1d77f91ca9447b1599e45` |
| Standalone oracle build | `a6ff571bb2fdfd6566898bd66cdeb94cb6e2f7ba7f165c06113e16732cc5a464` |
| Standalone oracle lockfile | `3552064d46dc17c2cff9a740be6356c2ae3a95507c00655c08adbebb842f3a3d` |

## Binary geometry and checksums

For D-V01, the [checked-range harness](kani/checked_range.rs) checks arbitrary `u64` offsets, lengths, and limits against a `u128` oracle. The [binary reader harnesses](kani/binary_geometry.rs) check the production field readers on arbitrary 0–64 byte windows, the full-width block-map size calculation, and admission of up to eight records from an exact block-map header. The block-map harness varies the declared `u32` count, available bytes, and budget. It checks the length and location of the record slice before the decoder allocates its record vector. The trailer harness varies full-width fork offsets, fork lengths, sector count, and body limit in a native 512-byte UDRO trailer. The other trailer fields retain their fixture values.

The [binary writer harnesses](kani/write_geometry.rs) check that the fixed-width writers change exactly the admitted field on 0–64 byte windows and that the production trailer encoder writes the header and geometry at their exact wire offsets. The field-writer harness assumes the caller provides enough bytes. The trailer harness checks that precondition for the fixed output layout by exercising the real encoder with arbitrary field values. The proof sources record the loop unwind bounds. The earlier symbolic experiment that included allocation and parsing of the full eight-record table was stopped. The admitted record slice is covered by Kani, and the full decoder is exercised by property tests and fuzzing.

Run the [geometry properties](binary_geometry.rs) with `PROPTEST_CASES=4096 cargo test --locked -p dmgez-udif --all-features --lib geometry_properties`. The table test exhausts count and budget through eight records and input lengths through one byte beyond a complete eight-record table. The trailer oracle uses widened endpoints and sector conversion, including a two-byte range starting at `u64::MAX`.

Kani rejected a scratch mutation that replaced checked endpoint addition with wrapping addition. The trailer regression also rejected that mutation. Rejecting a record count equal to the budget failed the one-record property case at the exact encoded length.

All seven D-V01 harnesses passed with Kani 0.68.0 and CBMC 6.11.0 on macOS 14.7.2 arm64. The all-feature UDIF suite passed 68 tests with `PROPTEST_CASES=4096`. Cargo-fuzz 0.13.2 ran the `udif` target for 20,000 inputs under AddressSanitizer, using seed `20261125` and eight repository DMG fixtures under the 64 KiB input limit. It produced no crash artifacts. [Fuzz target](../../../../fuzz/fuzz_targets/udif.rs)

The D-V02 Kani harnesses call the production `TableAdmission` transitions from `normalize` and the production `order_runs` index builder. Table admission covers up to eight records plus the terminator, up to 16 logical sectors, and the classic UDRO/UDZO run types. The proof checks exact logical and physical windows, one run per admitted sector, original record order, exclusion of comments, and preservation of the distinct zero and ignore kinds. The multi-record check uses `u16` record offsets and stored lengths with a full-width data window and table base. A separate compressed-run harness checks an arbitrary full-width physical origin, end, base, offset, and length against a `u128` oracle. Both assume the ordered data-fork endpoints established by trailer admission. The terminator harness varies all fields of a terminal record on a one-sector table.

The logical cross-table harness supplies two `Run` values with the shapes guaranteed by table normalization. It compares reverse-resource-order coverage to adjacent, disjoint intervals and reaches gap and overlap witnesses. An exhaustive production test enumerates all 2,025 pairs of half-open physical spans whose endpoints lie in `0..=8` bytes. Its byte-by-byte oracle distinguishes overlap, adjacent spans, and empty zero or ignore spans. In a scratch mutation that compared both span ends, the test caught reuse of `0..1`. The experimental Kani checks of the physical sort were stopped before completion. Plist and binary parsing, codec behavior, larger cross-table layouts, and independent and native format evidence remain separate from these checks.

Kani 0.68.0 with CBMC 6.11.0 on macOS arm64 verified the four harnesses in 94.16 seconds with four verification jobs. The eight-record composition took 92.11 seconds and reached all four covers. A scratch mutation allowed an incomplete logical table to finish successfully. Kani rejected it at the terminal coverage assertion.

The D-V03 [checksum-domain harness](checksum_domains.rs) compares the production writer, trailer and table declarations, and reader verification with a bitwise IEEE CRC32 oracle. It exercises one eight-sector mixed disk in UDRO and UDZO, writer chunk widths of one through eight sectors, and four input read strides. The UDZO one-sector case emits raw, compressed, and zero runs. A 257-sector raw run checks separately fragmented reads of the data fork and the logical table.

The native fixtures and their [recorded manifests](../../../../tests/fixtures/manifests) establish the data-fork, table, and master checksum domains. The harness verifies that explicit zero sectors contribute to the table CRC and ignored sectors are omitted. A four-table native fixture checks the master checksum of big-endian table CRCs in resource order. A changed raw sector with a recomputed data-fork CRC must still produce a logical-table checksum mismatch with the original and measured values. All six fixtures passed `hdiutil verify -nocache` on macOS 14.7.2 (23H311), and their SHA-256 digests matched before and after the checks.

Run the bounded checksum tests with `cargo test --locked -p dmgez-udif --test checksum_domains` from the workspace root.

The [plist admission tests](../tests/plist_admission.rs) cover D-UDIF-01-T04. They vary only the XML on the native tiny UDRO fixture. A safe unknown key survives in the descriptor and the logical bytes stay exact. Invalid base64 and a block-map count that disagrees with its decoded length fail admission.

## Writer cursor

The [writer cursor tests](writer_limits.rs) cover D-UDIF-03-T04 with a byte-counting sink. The public writer refuses a sector past the single-table limit before reading input or writing output. The tests seed the production writer at its final admissible chunk and at full-width cursor boundaries to check exact offsets and rejection before data or metadata writes. The seeded counters avoid streaming the preceding bytes.

The [cursor harnesses](kani/writer_cursor.rs) call the production output code. The data harness varies a full-width cursor and physical budget for one 512-byte sector. It checks the exact cursor advance and sink byte count on admission and zero sink calls on refusal. The metadata harness also varies the full-width XML length and checks the complete image endpoint before either metadata write. Both use an independent wire-size constant and reach exact-fit and overflow cases. The counting sink always accepts writes in full. The proof excludes error destructor behavior, codec internals, and short writes.

At `41d3afaa50ac87413c1fb55f83b84a35179d6953`, Kani 0.68.0 and CBMC 6.11.0 on macOS 14.7.2 (23H311) arm64 passed both harnesses and reached all six covers. Moving the sink write ahead of the check in a scratch copy triggered the zero-call assertion. Earlier experiments were stopped after the verifier repeatedly unwound error destructors. The final harness borrows the result variant without running its destructor. Run the harnesses with `cargo kani --manifest-path crates/formats/dmgez-udif/Cargo.toml --no-default-features --features write --harness writer_cursor_proof --output-format terse --jobs 2`.

The Git source archive covers `dmgez-udif`, `dmgez-core`, and `Cargo.lock` at that revision. Its SHA-256 is `487590da655432b8801970ab2e1b732c8e61c8286b3b118d4b4fa925b0ad373e`. Local normalized logs are under `target/verification-evidence/udif-v0.1.0/`.

| Run | Log SHA-256 |
| --- | --- |
| Kani cursor harnesses | `2a4a0cc296d3c29618a18179e3e9a18b7b05c23852a7549bdf569f22f127fa59` |
| Expected write-before-admission counterexample | `60c5a9dfdbd2deb0d5f48b59cef3971604b05aa800832d9316352eb940af892f` |
| 78 UDIF tests, Clippy, formatting, and architecture | `7d490db1d8fdef2195d2b46fa2ec5a38415aaed66faa7d30c54cf95b68037b9c` |
| UDRO and UDZO project smoke with Apple and 7-Zip oracles | `4492b414283d3f782bedccb1df572e7489885db8c65afe8c50d85539b3c2e50e` |

## Indexed reads

The indexed-read harnesses cover routing for D-V04 through the production cursor called by `Image::read_exact_at`. They assume a contiguous logical partition and an admitted request. There are zero to eight runs, with arbitrary strictly increasing `u64` endpoints and reverse backing order. The neighboring endpoints identify the unique starting ordinal. The step harness checks the exact half-open fragment, advancement to the following run when bytes remain, and termination on an empty request. The unwind bound is 10. Reachability checks include a later starting run, a request at the disk end, and a fragment that ends at a run boundary while the request continues.

The [output-window proof](kani/read_ranges.rs) calls the production conversion from a logical fragment to a caller-buffer range. It varies full-width `u64` endpoints and the output length at the verifier's native `usize` width. A widened independent oracle checks admission, translated endpoints, and length. The proof reaches a valid window ending at the output boundary and rejected windows on either side of the request. Shifting the output start by one byte in a scratch mutation failed the admission and endpoint assertions. The streaming test also checks an unaligned positioned read that crosses compressed and zero runs against the exact input bytes. [Streaming regression](../tests/streaming.rs)

The routing checks enable only `read`. The [reader limit tests](reader_limits.rs) exercise the production reader with oversized XML and compressed-run geometry under permissive caller limits. The reader rejects these sizes before allocating their buffers and accounts for the spare output byte used by the zlib and LZMA adapters. The native UDZO fixture verifies that zlib consumes exactly the stored stream and produces exactly the declared length. A synthetic unknown run returns `UnsupportedRun` and leaves its output byte untouched. AddressSanitizer completed 20,000 inputs against the `udif` fuzz target with seed `20261126` and the eight repository DMG fixtures under its input limit. Codec internals, I/O failures, and verification outside the stated bounds remain separate.

The boundary mutation witness extends the current run by one byte before clipping the fragment to the request end. In an isolated source copy, Kani rejected the mutated `ReadCursor::next` at the exact-intersection assertion.

The [v0.1.0 evidence records](evidence) identify the tested revisions, methods, bounds, and log hashes. `source_hash` covers the Git tar archive of this crate, `dmgez-core` and the workspace lockfile at the recorded revision. `fixture_hash` covers the tracked repository and crate fixture trees. Local logs live under `target/verification-evidence/udif-v0.1.0/` with private paths replaced by placeholders. The 78-test run used 4,096 property cases and seed `20261126`. The AddressSanitizer run processed 20,000 inputs from nine seed fixtures and left no crash artifacts. The local Apple Silicon candidate replayed the native UDRO and UDZO checks with p7zip 17.05 and macOS 14.7.2.

## Concurrent decode cache

The [cache tests](decode_cache.rs) exercise D-UDIF-04-T04 against the production reader. The Loom build substitutes the cache mutex and fixture synchronization. The encoder, parser, decoder and cache policy use their normal implementations. Two readers overlap within two resident runs to check one inflate per run. A separate case overlaps across three runs to force eviction. Raw fixture bytes are the read oracle. Further checks establish least-recently-used eviction, count the spare zlib output byte in the capacity limit, and compare different images with equal segment identifiers.

At `b0b61d00a2c14391d9f7315b8f8c93a0d68b9d1d`, all four cases passed with Loom 0.7.2 on macOS 14.7.2 arm64. Removing the retained-capacity check in a scratch copy caused the capacity test to fail. The native build passed 82 UDIF tests. A composed CLI using HFS+ `a1b870f` reproduced all four project digests and passed Apple and 7-Zip verification. [Structured evidence](evidence/D-V04.json)

```sh
RUSTFLAGS="--cfg dmgez_loom" cargo test --locked -p dmgez-udif \
  --no-default-features --features read,write,zlib --lib decode_cache_model
```

Install Kani 0.68.0 and run `cargo kani setup` before running `bash check.sh` from this directory. The checker also accepts an absolute path to `check.sh`.

The [ordered batch model](models/README.md) covers the TLC portion of D-V07, including batch admission, input-order writes, cancellation, and credit release.
