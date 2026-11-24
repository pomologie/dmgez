# UDIF verification

The checked-range Kani harness covers the arithmetic portion of D-V01. It calls the production range check with arbitrary `u64` offsets, lengths, and limits. A `u128` reference checks admission and both range endpoints. Binary-layout checks and native evidence for D-V01 remain tracked in the repository obligation registry.

The indexed-read harnesses cover routing for D-V04 through the production cursor called by `Image::read_exact_at`. They assume a contiguous logical partition and an admitted request. There are zero to eight runs, with arbitrary strictly increasing `u64` endpoints and reverse backing order. The neighboring endpoints identify the unique starting ordinal. The step harness checks the exact half-open fragment, advancement to the following run when bytes remain, and termination on an empty request. The unwind bound is 10. Reachability checks include a later starting run, a request at the disk end, and a fragment that ends at a run boundary while the request continues.

The routing checks enable only `read`. Parser admission, output-buffer arithmetic, payload decoding, unknown encodings, I/O failures, and verification outside the stated finite bounds require separate evidence in the obligation registry.

The boundary mutation witness extends the current run by one byte before clipping the fragment to the request end. In an isolated source copy, Kani rejected the mutated `ReadCursor::next` at the exact-intersection assertion.

Install Kani 0.68.0 and run `cargo kani setup` before running `bash check.sh` from this directory. The checker also accepts an absolute path to `check.sh`.

The [ordered batch model](models/README.md) covers the TLC portion of D-V07, including batch admission, input-order writes, cancellation, and credit release.
