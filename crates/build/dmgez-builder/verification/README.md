# Builder verification

## Staged no-clobber

The [publication model](models/Publication.tla) covers the staged no-clobber path of D-V06 and D-BUILD-05. It explores two attempts targeting the same destination, initially vacant or occupied. Each attempt can fail at any stage from admission through commit. The model allows one cancellation request and at most one crash, either of one process or of the whole system. The process crash can occur after the destination becomes visible and before the caller receives success. After power loss, the model explores whether the destination directory entry survives and whether owned staging files remain. Both attempts are terminal after power recovery.

- `CompleteCheckedPublication` requires each visible new image to contain the complete staged data, with a successful file sync and final check.
- `NoClobber` requires an existing destination to survive and allows at most one successful installation into the directory.

The model assumes a stable destination directory, distinct owned staging files, and a host primitive that atomically installs a new entry only when vacant. A successful file sync establishes the staged-data premise. The builder leaves parent-directory durability to the host filesystem, so the model permits a newly installed entry to disappear after power loss. An initially occupied entry is assumed durable. A cancellation request is observed at admission, writing, or the final check. After the final check succeeds, the publication call can complete even if cancellation arrives before commit. The model encodes safety without fairness assumptions and enables stuttering only after both attempts end and restart, if needed, completes.

The builder uses `publish_new` for this profile. Source identity and directory confinement depend on the host discovery layer. Host primitive qualification, crash replay, and deterministic fault injection at each boundary remain separate D-V06 evidence. [Builder execution](../src/execution.rs) · [Shared output publication](../../../foundation/dmgez-core/src/output.rs) · [Obligation registry](../../../../verification/obligations.json)

### Model actions and production checks

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

## Checked atomic replacement

The [replacement model](models/CheckedReplace.tla) covers two attempts with one advisory directory lock on a trusted local filesystem. Cooperating writers hold the lock from capture through commit or abort. A contender can return `Busy` while the lock is held, or acquire it after release and capture a fresh predecessor. Each attempt may fail from lock acquisition through rename. The model permits one cancellation request and one process crash. Death releases the lock, may leave an owned stage, and preserves any already visible commit. Power-loss durability remains a separate host concern.

The initial target can be absent, a regular predecessor, a source alias, or a non-regular entry. The model also exercises rejection of a protected source directory. A writer authorizes the captured directory and target, then writes, syncs, rewinds and validates its stage. It rechecks the stage identity and fingerprint, directory identity, and target fingerprint in that order. A target token represents the complete fingerprint, so the `ChangedTarget` token can represent either an inode replacement or a metadata change to the same inode. An initially absent target uses the native no-clobber commit, which tests actual vacancy at the syscall. A present target uses ordinary atomic rename against the captured directory descriptor after the final check. [Replacement implementation and host contract](../../../foundation/dmgez-core/src/output/replace.rs)

- `ExclusivePublisher` requires every attempt in a locked phase to hold the directory lock.
- `AuthorizedPublication` checks each rename against its actual predecessor, directory, and stage at the syscall. The stage must still be the owned, complete, synced and validated one.
- `AbortsPreserveOutput` checks that the visible target follows the history of external edits and installs. Attempts ending in failure or lock contention contribute no install.
- `ForeignStagePreserved` requires rejected stage substitutions to remain in the namespace rather than being removed during cleanup.

`CheckedReplace.cfg` checks cooperating writers in the trusted directory. `CheckedReplaceObserved.cfg` additionally allows a single stage, directory, or target change before its corresponding recheck. That configuration also allows one late target creation after a vacant target passes its final check. The native no-clobber branch preserves the resulting entry. Host rechecks and the protocol's guards are distinct model steps, so TLC can exercise the changes at their actual boundaries.

`CheckedReplaceUntrusted.cfg` injects one noncooperating target change after a present target passes the final recheck. The same production-modeled rename proceeds and violates `AuthorizedPublication` because its actual predecessor differs from the captured one. This counterexample marks the trusted-directory boundary of advisory locking and ordinary rename. Descriptor-relative addressing and fingerprint checks alone cannot make the final check and rename conditional on the same inode.

The native Rust tests exercise replacement with old handles kept open, protected and non-regular destinations, rejection after precommit target or directory changes, cooperative lock contention, stage substitution, and verification or write failure preserving the predecessor. The builder tests cover the optional overwrite profile and source-alias authorizer. The late present-target mutation is a model counterexample outside the production contract. [Core replacement tests](../../../foundation/dmgez-core/tests/replacement.rs) · [Builder execution tests](../tests/execution.rs)

## Reproduction

Supply the official TLA+ Tools v1.8.0 JAR and run the checker from the repository root. It verifies the release digest before running TLC, uses one breadth-first worker with seed 1 and fingerprint 0, and checks the exact invariant diagnostics and trace witnesses expected from the negative configurations. TLC's default deadlock check remains enabled. [Pinned release asset and SHA-256](https://github.com/tlaplus/tlaplus/releases/expanded_assets/v1.8.0)

```sh
TLA2TOOLS_JAR=/path/to/tla2tools.jar bash crates/build/dmgez-builder/verification/check.sh
cargo nextest run --locked -p dmgez-core -p dmgez-builder --features dmgez-core/staging \
	--test publication --test replacement --test execution
```

Local evidence uses Java Temurin 21.0.8 on macOS 14.7.2 arm64 and TLC revision `bc1b506`. The initial no-clobber evidence used a working tree based on dmgez `11c6403` and passed the five targeted Rust tests. The expanded check used the builder replacement commit `dc9d0f0` and passed all 18 targeted Rust tests. Each negative search stopped at its first counterexample.

| Configuration | States generated | Distinct states | Depth | Result |
| --- | ---: | ---: | ---: | --- |
| `Publication` | 19,310 | 3,120 | 16 | Complete, exit 0 |
| `PublicationEnospc` | 469 | 181 | 6 | Partial unchecked image became visible, exit 12 |
| `PublicationRace` | 11,065 | 1,976 | 15 | Second writer replaced the winner, exit 12 |
| `CheckedReplace` | 14,576 | 6,066 | 28 | Complete, exit 0 |
| `CheckedReplaceObserved` | 35,342 | 14,946 | 28 | Complete, exit 0 |
| `CheckedReplaceUntrusted` | 2,906 | 1,361 | 14 | Present target changed between check and rename, exit 12 |

## Reproducible unsigned project builds

The [D-PERF-02-T02 harness](check-reproducibility.py) uses separate CLI processes and freshly materialized source trees for serial, 1/2/4/8 requested workers, and a serial repeat. The trees alternate creation order and include nested directories, a Unicode file name, a dotfile, and a mixed-entropy payload. The project adds an Applications symlink and Finder layout. It uses an explicit build epoch, fixed modes, and explicit omissions for host ownership, timestamps and extended metadata. Each process receives a `SOURCE_DATE_EPOCH` that differs from the project's explicit epoch.

The check requires multiple batches of UDZO runs containing both raw and zlib data. The baseline passes container verification, and its HFS+ reader returns the exact source payload. Each repeat must pass semantic HFS+ diff, including FinderInfo, and match the baseline image SHA-256. A separate build with a different explicit epoch must yield a different SHA-256. On macOS, `--native` additionally runs `hdiutil verify` and compares the SHA-256 of its UDTO export against `dmgez unwrap-raw`.

Run from the repository root with Python 3.11 or later and source or unpacked HFS+ packages, using the [local composition tool](../../../../README.md#build-from-a-toml-project).

```sh
python3 tools/compose_hfsplus_cli.py --hfsplus ../hfsplus --features hfsplus-read,diff
python3 crates/build/dmgez-builder/verification/check-reproducibility.py \
	--dmgez target/hfsplus-cli/debug/dmgez --native
```

On macOS 14.7.2 arm64 (23H311), all six equivalent builds matched using dmgez `5e07648`, HFS+ packages from `fb11b35`, Rust 1.99.1, Rayon 1.12.0 and zlib-rs 0.6.8. The 1 MiB HFS+ image had 11 raw runs and 17 zlib runs. [Acceptance scenario](../../../../docs/reference/test-catalog.json)

| Artifact | SHA-256 |
| --- | --- |
| Unsigned UDZO image | `5b9dc6d7c2a3021c3e9dcc0f7a853d0faaf1377aee84a5318af8b8040d2d1899` |
| UDZO image with explicit epoch increased by one second | `247ce6e99798421a68fb6e07d3d53515051c3a11ef77af234d7225b00fd49816` |
| Decoded disk bytes, confirmed by native UDTO export | `e72a459fd34a0aa5fba01b56c580ade256ac01749380e0b01c12d9fb84dacfcf` |

Cross-host runs and the larger performance and memory workloads remain open under D-PERF-02-T01 and D-PERF-02-T03. The matrix exercises production ordering with one bounded fixture. D-V08's linked proof still needs to cover arbitrary completion order within its declared bounds. [Verification obligations](../../../../verification/obligations.json)
