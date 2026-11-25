# Extraction name verification

The D-V10 harnesses call the production bytewise validation used by `HostName::admit` and the path-byte charging helper used by `preflight_names`. The component proofs compare exact acceptance and error classification against a reference grammar for every byte sequence of zero to twelve bytes and every `usize` byte limit. `HostName::admit` passes the decoded UTF-8 bytes to this validator and retains the original spelling. The portable reference lists its ASCII exclusions and spells out literal and numbered device stems. It compares against the stem before the first dot, ignoring spaces at the end of that stem. The component unwind bound is 16.

The path harnesses compare the production separator and length charges against widened arithmetic. The single step admits arbitrary `usize` inputs and checks overflow, and the chain considers zero to eight ordered components of one to twelve bytes each, checking every prefix against the limit with an unwind bound of 10. Each component in a chain represents an already admitted name. Reachability checks exercise accepted Unicode and ASCII, traversal components, separator and device rejections, byte limits, overflow, and empty and eight-component chains.

The lexical model assumes the caller's qualified host name profile. UTF-16 decoding, collision detection, topology, host pathname interpretation, symlink handling, handle-relative publication, and rename races require separate evidence. D-V10 remains tracked in the repository obligation registry.

Install Kani 0.68.0 and run `cargo kani setup` before running `bash check.sh` from this directory. The checker also accepts an absolute path to `check.sh`.

On `aarch64-apple-darwin`, Kani 0.68.0 with CBMC 6.11.0 passed all four harnesses and reached all sixteen cover conditions. Removing the space trim from the device-name stem in a scratch copy failed the portable error-equivalence assertion. The counterexample was `conIn$ .@oo` with an unrestricted byte limit.
