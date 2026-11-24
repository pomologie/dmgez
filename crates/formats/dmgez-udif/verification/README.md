# UDIF verification

The checked-range Kani harness covers the arithmetic portion of D-V01. It calls the production range check with arbitrary `u64` offsets, lengths, and limits. A `u128` reference checks admission and both range endpoints. Binary-layout checks and native evidence for D-V01 remain tracked in the repository obligation registry.

Install Kani 0.68.0 and run `cargo kani setup` before running `bash check.sh` from this directory. The checker also accepts an absolute path to `check.sh`.
