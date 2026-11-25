# UDIF verification

The checked-range Kani harness covers the arithmetic portion of D-V01. It calls the production range check with arbitrary `u64` offsets, lengths, and limits. A `u128` reference checks admission and both range endpoints. Binary-layout checks and native evidence for D-V01 remain tracked in the repository obligation registry.

The D-V02 Kani harnesses call the production `TableAdmission` transitions from `normalize` and the production `order_runs` index builder. Table admission covers up to eight records plus the terminator, up to 16 logical sectors, and the classic UDRO/UDZO run types. The proof checks exact logical and physical windows, one run per admitted sector, original record order, exclusion of comments, and preservation of the distinct zero and ignore kinds. The multi-record check uses `u16` record offsets and stored lengths with a full-width data window and table base. A separate compressed-run harness checks an arbitrary full-width physical origin, end, base, offset, and length against a `u128` oracle. Both assume the ordered data-fork endpoints established by trailer admission. The terminator harness varies all fields of a terminal record on a one-sector table.

The logical cross-table harness supplies two `Run` values with the shapes guaranteed by table normalization. It compares reverse-resource-order coverage to adjacent, disjoint intervals and reaches gap and overlap witnesses. An exhaustive production test enumerates all 2,025 pairs of half-open physical spans whose endpoints lie in `0..=8` bytes. Its byte-by-byte oracle distinguishes overlap, adjacent spans, and empty zero or ignore spans. In a scratch mutation that compared both span ends, the test caught reuse of `0..1`. The experimental Kani checks of the physical sort were stopped before completion. Plist and binary parsing, codec behavior, larger cross-table layouts, and independent and native format evidence remain separate from these checks.

Kani 0.68.0 with CBMC 6.11.0 on macOS arm64 verified the four harnesses in 94.16 seconds with four verification jobs. The eight-record composition took 92.11 seconds and reached all four covers. A scratch mutation allowed an incomplete logical table to finish successfully. Kani rejected it at the terminal coverage assertion.

The indexed-read harnesses cover routing for D-V04 through the production cursor called by `Image::read_exact_at`. They assume a contiguous logical partition and an admitted request. There are zero to eight runs, with arbitrary strictly increasing `u64` endpoints and reverse backing order. The neighboring endpoints identify the unique starting ordinal. The step harness checks the exact half-open fragment, advancement to the following run when bytes remain, and termination on an empty request. The unwind bound is 10. Reachability checks include a later starting run, a request at the disk end, and a fragment that ends at a run boundary while the request continues.

The routing checks enable only `read`. Parser admission, output-buffer arithmetic, payload decoding, unknown encodings, I/O failures, and verification outside the stated finite bounds require separate evidence in the obligation registry.

The boundary mutation witness extends the current run by one byte before clipping the fragment to the request end. In an isolated source copy, Kani rejected the mutated `ReadCursor::next` at the exact-intersection assertion.

Install Kani 0.68.0 and run `cargo kani setup` before running `bash check.sh` from this directory. The checker also accepts an absolute path to `check.sh`.

The [ordered batch model](models/README.md) covers the TLC portion of D-V07, including batch admission, input-order writes, cancellation, and credit release.
