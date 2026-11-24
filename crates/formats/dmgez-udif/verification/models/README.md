# UDIF ordered batch model

`OrderedBatch.tla` covers the TLC portion of D-V07 and traces to D-PERF-01. `ordered-batch.cfg` explores one to four logical chunks, one to three workers, zero to four whole-chunk credit units, and both automatic and explicit width selection. The insufficient-budget cases start with zero held credits and fail before reading. The model checks deadlocks and eventual termination under weak fairness of operation steps. Successful and failed return states permit terminal stuttering.

## Production mapping

| Model action | Production boundary |
| --- | --- |
| `Admit` | `WriteOptions::admit` and `Scheduler::new` choose width, charge buffers and codec workspace, and create the private pool when needed. |
| `Fill` and `Read` | `write_image` checks cancellation, reserves a slot before allocating and reading, then adds a raw chunk to the current batch. |
| `StartEncode` and `CompleteEncode` | `RawChunk::encode` checks cancellation on entry and returns a stored chunk or an error. The model records completions in arbitrary order. |
| `Encode` transition to `consume` | `Scheduler::encode` collects the entire indexed Rayon iterator before the writer can consume any result. |
| `Consume` and `Write` | `write_image` checks cancellation before each indexed result. `DataFork::write` consumes a successful result and releases its reservation. |
| `Abort` | An early return drops outstanding raw or stored chunks and all corresponding reservations. A worker error is observed at its input ordinal after the batch joins. |
| `CheckEof` and `Finish` | `write_image` checks the declared input length. `DataFork::finish` writes final metadata, flushes, checks cancellation, and returns a report on success. |

Each credit represents one production worst-case chunk charge. The model conservatively holds that credit even when an encoded result occupies less memory or an error has dropped its raw buffer. The automatic worker input is the host limit already capped by the production policy. The model treats encoding, source I/O, and sink I/O as opaque operations that return; liveness assumes their calls eventually complete. Cancellation is cooperative at the sampled boundaries. An in-progress write can finish before cancellation is sampled again. A failed stream may contain a partial prefix. Chunk order is logical because the writer may merge adjacent zero runs. Byte-level codec behavior, allocator and Rayon internals, panic handling, cancellation latency, and destination publication are outside this model.

## Reproduction and evidence

Use the official TLC v1.8.0 JAR and Java 21. The checker verifies the published SHA-256 and runs TLC with one worker, a fixed seed and fingerprint index, and deadlock checking enabled. [TLA+ Tools v1.8.0 release assets](https://github.com/tlaplus/tlaplus/releases/expanded_assets/v1.8.0)

```sh
TLA2TOOLS_JAR=/path/to/tla2tools.jar bash verification/models/check-ordered-batch.sh
```

Run the command from `dmgez-udif`, or invoke the checker by its absolute path. The two mutation configurations must exit with the named invariant violations. The completion-order mutation waits for both workers to finish and then consumes chunk 2 before chunk 1, which violates `OrderedConsumption`. The lost-credit mutation returns after the first source read fails and leaves its credit held, which violates `CreditsReleasedOnTermination`.

On macOS 14.7.2 arm64 with Temurin 21.0.8, the positive configuration visited 23,192 generated states and 13,238 distinct states from 120 initial configurations. TLC completed the search at depth 41 with no invariant, deadlock, or temporal errors. The completion-order witness reached the violation in 14 states, and the lost-credit witness in four.

The production mappings come from `writer.rs` and `writer/scheduler.rs` at dmgez revision `11c6403d29db73489cdfe73e5fbe2403f5645b3a`. At that revision, the `streaming` integration test passed four cases with all features and three with only `write` enabled. These exercise serial and parallel output across worker counts, short and long input, cancellation, sink failure, and insufficient credit before reading.

## Production I/O replay

The `ordered_faults` integration test replays the model's observable I/O boundaries through `write_image`. It covers all 12 combinations of one to four distinguishable UDRO sectors and explicit widths from one to three, using real Rayon pools at widths two and three. The successful trace checks that the writer fills each batch and emits sectors in input order before admitting the next batch. [Ordered fault tests](../../tests/ordered_faults.rs)

The replay injects a single error or cancellation at each of 108 boundaries across those combinations. It checks the exact trace and accepted byte prefix, including the EOF probe after the final payload write and cancellation observed after a successful flush. It also checks 30 short-write-then-error traces and 12 insufficient-budget admissions. The observable trace covers source reads, data writes, EOF, resource and trailer writes, and flush. Worker completion interleavings, direct reservation drops, Loom, and Shuttle require separate evidence.

```sh
cargo test --locked -p dmgez-udif --no-default-features --features write,parallel \
	--test ordered_faults
```

All three tests passed with `write,parallel` alone and with all features enabled. A scratch mutation at revision `e213ee5` reversed the indexed parallel iterator before collection. The gate rejected its two-chunk, two-worker trace at the first emitted sector, matching the ordering violation in `completion-order.cfg`.
