# Builder publication verification

The [publication model](models/Publication.tla) covers the staged no-clobber path of D-V06 and D-BUILD-05. It explores two attempts targeting the same destination, initially vacant or occupied. Each attempt can fail at any stage from admission through commit. The model allows one cancellation request and at most one crash, either of one process or of the whole system. The process crash can occur after the destination becomes visible and before the caller receives success. After power loss, the model explores whether the destination directory entry survives and whether owned staging files remain. Both attempts are terminal after power recovery.

- `CompleteCheckedPublication` requires each visible new image to contain the complete staged data, with a successful file sync and final check.
- `NoClobber` requires an existing destination to survive and allows at most one successful installation into the directory.

The model assumes a stable destination directory, distinct owned staging files, and a host primitive that atomically installs a new entry only when vacant. A successful file sync establishes the staged-data premise. The builder leaves parent-directory durability to the host filesystem, so the model permits a newly installed entry to disappear after power loss. An initially occupied entry is assumed durable. A cancellation request is observed at admission, writing, or the final check. After the final check succeeds, the publication call can complete even if cancellation arrives before commit. The model encodes safety without fairness assumptions and enables stuttering only after both attempts end and restart, if needed, completes.

The modeled path is [builder execution](../src/execution.rs) through [shared output publication](../../../foundation/dmgez-core/src/output.rs). Atomic replacement requires source-alias protection and is rejected by the current builder. Source identity and directory confinement depend on the host discovery layer. Host primitive qualification, crash replay, and deterministic fault injection at each boundary remain separate D-V06 evidence. [Obligation registry](../../../../verification/obligations.json)

## Model actions and production checks

| Model action | Production operation |
| --- | --- |
| `Admit` | Output intent and independent verifier admission, source identity and cancellation checks |
| `CreateStage` | `NamedTempFile::new_in` in the destination parent |
| `WritePrefix`, `FinishWrite`, `StorageFull` | Staged writer emits an incomplete image, completes it, or returns an I/O failure |
| `SyncStage`, `RewindStage` | `sync_all` followed by rewind to the start of the staged image |
| `CheckStage`, `ObserveCancel` | Read-only optional verifier followed by compulsory source identity and cancellation checks |
| `StageError`, `AbandonStage` | Return failure and attempt cleanup of the owned staging entry |
| `CommitStage`, `ReturnSuccess` | `persist_noclobber` installs the staged output, then the builder reports success |
| `CrashProcess`, `LosePower`, `Restart` | Environment steps that distinguish visible directory state, synced staged data, and process progress |

`PublicationEnospc.cfg` admits a broken commit after a prefix write and ENOSPC. TLC must reject `CompleteCheckedPublication` with a trace that reaches `WriteFailed`. The production write-fault test injects `StorageFull` after a data write and checks the original destination, returned error, and directory entries.

`PublicationRace.cfg` lets the second writer clobber a winning publication. TLC must reject `NoClobber` with both writers in the commit history. The production race test creates the competing destination during validation and checks that the loser returns `AlreadyExists`, preserves the winner's bytes, and removes the owned staging entry. [Shared publication tests](../../../foundation/dmgez-core/tests/publication.rs)

The builder tests observe optional-verifier success and failure, cancellation before staging, and an invalid caller-owned stream after a short sink write. These tests exercise production behavior at selected observable boundaries. The model's intermediate steps and crash schedules still need instrumented Rust replay. [Builder execution tests](../tests/execution.rs)

## Reproduction

Supply the official TLA+ Tools v1.8.0 JAR and run the checker from the repository root. It verifies the release digest before running TLC, uses one breadth-first worker with seed 1 and fingerprint 0, and checks the exact invariant diagnostic and trace witness for both mutations. TLC's default deadlock check remains enabled. [Pinned release asset and SHA-256](https://github.com/tlaplus/tlaplus/releases/expanded_assets/v1.8.0)

```sh
TLA2TOOLS_JAR=/path/to/tla2tools.jar bash crates/services/dmgez-builder/verification/check.sh
cargo nextest run --locked -p dmgez-core -p dmgez-builder --features dmgez-core/staging \
	--test publication --test execution
```

Initial local evidence used Java Temurin 21.0.8 on macOS 14.7.2 arm64, TLC revision `bc1b506`, and a working tree based on dmgez `11c6403`. The publication model completed and all five targeted Rust tests passed. Both mutated searches stopped at their first counterexample.

| Configuration | States generated | Distinct states | Depth | Result |
| --- | ---: | ---: | ---: | --- |
| `Publication` | 19,310 | 3,120 | 16 | Complete, exit 0 |
| `PublicationEnospc` | 469 | 181 | 6 | Partial unchecked image became visible, exit 12 |
| `PublicationRace` | 11,065 | 1,976 | 15 | Second writer replaced the winner, exit 12 |
