# dmgez verification plan

Immutable image generation and untrusted reading are the first priority. Model ordering/credits and publication before the parallel builder release, not after it. Do not require APFS or mutable filesystem proofs for the default CLI. Native format checks, data preservation, and proofs of pure Rust are separate evidence. Signing, mount, remote and MCP get their own approval/state gates.

Read the shared ecosystem verification standard [EVS-1](ECOSYSTEM_VERIFICATION.md). The [obligation index](../../verification/obligations.json) defines properties, bounds, negative witnesses, methods, and current status. Implemented proof artifacts live in the owning crate’s `verification/` directory. Required proofs and tests must pass before the affected capability ships.

## v0.1.0 release evidence

```sh
cargo run --locked -p xtask -- release-evidence \
  --root /path/to/clean/candidate \
  --bundle /path/to/evidence/bundle.json
```

The [release profile](../../verification/release-profile.json) lists the required invocation groups for the activated obligations. Supply one [schema v1](../../verification/evidence.schema.json) record per profile and obligation in a JSON array, with each transcript named by a path relative to the bundle. A release record's `source_hash` is the SHA-256 of `git -c tar.umask=0002 archive --format=tar HEAD` from the candidate. Commands use the exact profile arguments and replace disposable paths with the profile's placeholders. Run each Kani group serially to retain the per-harness result and cover lines in order. The checker requires the clean candidate commit, the complete activated matrix, the declared command and features, matching transcript hashes, meaningful Kani assertions and the expected covers. The release process supplies the transcripts from the actual invocation. The checker validates their consistency and scope.

The clean candidate `e048add07f0932a47ddd70c5ee8b7f75a85f5206` passed all 18 records on macOS 14.7.2 arm64. The run included 16 Kani harnesses, 98 UDIF tests, four Loom scenarios, 20,000 AddressSanitizer inputs, both named Rust reference decoders, and native read-only HFS+ attachment and filesystem checking. The checked-range harness reached an exact-fit request and a rejected overflowing request. The executable rejected missing, stale, dirty, timed-out, unsupported and wrong-feature records, a command that suppressed default Kani checks, a hash mismatch and a fresh transcript with missing covers.

Local logs and the bundle are under `target/verification-evidence/v0.1.0-verification/e048add-release/`.

| Artifact | SHA-256 |
| --- | --- |
| Qualified bundle | `3fb452a2ff3399d0eb92c1ef0142011e8a21559f2796acdfe288ee64ffa6d765` |
| Validator success | `65630e1a30ef97fa723334eba25d529ec44c0cc156869517f3e3c0cfac133823` |
| Validator rejections | `76cf263f674a440d90ad45acb0a68027ae11d59e1ffb2033d30c9d2972b5458e` |
| Workspace nextest | `99bbf01c88b1f0408293e69791caeee34c0907fe5b9eb23714e512d9231c66cd` |

Nextest passed 874 workspace tests with localhost access and skipped eight. The negative architecture checks rejected a CLI dependency on the GUI runtime and a 501-line source file. The coverage audit at `9e04fb0` measured 86.53% lines across the four selected builder, config, core and UDIF packages, and 89.87% lines and 75.00% branches in the separate UDIF run. The local coverage driver was 0.9.1. The coverage scope and uncovered critical branches remain open under D-VER-01. Its normalized coverage records are indexed in `target/verification-evidence/v0.1.0-verification/9e04fb0-apparatus-49a997ddf1a4db762b344d84861e15604b8314e6914007a2f6c535e7030c26e2.json`.

The UDIF admission and reader follow-up compared `a6d6c64` with `62a85b4` on macOS arm64. Both runs used `cargo test --locked -p dmgez-udif --all-features`, nightly `2026-11-04` branch instrumentation, its bundled LLVM 23.1.5 tools, and 4,096 property cases with seed `20261126`. The tests passed 98 and 115 cases respectively. The totals cover the 30 instrumented files under `dmgez-udif/src/`.

| Source | Baseline lines | Candidate lines | Baseline branches | Candidate branches |
| --- | ---: | ---: | ---: | ---: |
| `index.rs` | 87.88% | 98.48% | 59.62% | 88.46% |
| `trailer.rs` | 94.19% | 100.00% | 58.33% | 91.67% |
| `reader.rs` | 88.98% | 95.10% | 87.50% | 95.83% |
| `reader/cache.rs` | 89.09% | 98.18% | 83.33% | 83.33% |
| UDIF crate source | 89.87% | 91.87% | 74.78% | 80.31% |

D-VER-01 remains open. The index and cache are below the proposed 90% branch floor for critical code, and the workspace coverage scope still needs review. The table decoder rejects zero records before index normalization. D-V04 records separate Kani evidence for the positioned-output range guard. The per-file export, test logs, and measurement script are in `target/verification-evidence/v0.1.0-verification/62a85b4-coverage/`. The SHA-256 of `udif-coverage.json` is `b3f1070e5367ee9b7e757d08320d6d7b40b332d8ed1f0213fc09fabe41f1c5a5`.

Three targeted mutation checks ran on macOS arm64 against `f5e2fb4` with Rust 1.99.1 and the reader-only feature set. Each focused test passed against the original source and failed its assertion after one change in a disposable checkout. The trailer test caught an unexpected XML read after encountering unsupported flags, the cache test caught acceptance of an allocation one byte over its reservation, and the index test caught the loss of the specific empty-run refusal. The exact edits, test commands, and transcript hashes are in `target/verification-evidence/v0.1.0-verification/f5e2fb4-mutations/summary.json` with SHA-256 `57f3b9ca844e2550d43978e876f87897965a9d2a86809363f8812c3e489956fa`.

The hosted Linux coverage and minimum-Rust job passed at `ffb76e4`. The all-feature workspace report covered 27,741 of 32,755 instrumented lines (84.69%) and 41,574 of 50,363 regions (82.55%). This job did not instrument branches. Rust 1.98.0 compiled the locked all-feature workspace on the same host. [Hosted coverage and minimum-Rust job](https://github.com/pomologie/dmgez/actions/runs/45790210058/job/139807027014)

## Proof obligations

| ID | Activation | Scope | Required evidence |
|---|---|---|---|
| D-V01 | 0.1.0 | UDIF read/write | kani, property, fuzz |
| D-V02 | 0.1.0 | UDRO/UDZO | kani, property, differential, native |
| D-V03 | 0.1.0 | UDRO/UDZO | property, differential, native |
| D-V04 | 0.1.0 | UDIF reader | kani, property, fuzz, differential, loom |
| D-V05 | 0.2.0 | builder | property, differential, native |
| D-V06 | 0.2.0 | builder publication | tlc, fault, integration |
| D-V07 | 0.2.0 | parallel builder | tlc, loom, shuttle, property, fault |
| D-V08 | 0.3.0 | reproducible build | lean-linked, property, differential, native |
| D-V09 | 0.4.0 | inspection/partition windows | kani, property, differential |
| D-V10 | 0.4.0 | extract | kani, property, fuzz, fault, integration |
| D-V11 | 0.5.0 | GUI project/edit | property, state-machine, snapshots, integration |
| D-V12 | 0.6.0 | signing and release | tlc, state-machine, fault, native |
| D-V13 | 0.7.0 | conversion/edit/cache | property, fuzz, fault, differential |
| D-V14 | 0.8.0 | mount and install | tlc, state-machine, fault, integration, native |
| D-V15 | 0.9.0 | remote and MCP | tlc, property, fuzz, fault, integration |
| D-V16 | 1.0.0 | UDIF reader/writer and sealed images | lean-linked, property, differential |
| D-V17 | 1.0.0 | architecture and release | integration, mutation |
| D-V18 | stretch | advanced/legacy/encryption/writable mounts | property, fuzz, differential, native, fault |

## Invariant and proof details

### D-V01 — UDIF read/write
**Target:** dmgez-udif / checked ranges, endian codecs.\
**Property:** Every accepted slice, sector conversion, count*record-size and offset+len is representable and within the admitted parent range; every rejected input has no dereference/allocation based on invalid geometry.\
**Assumptions and initial bounds:** Kani full u64 operands; arbitrary 0..64-byte buffer windows and 0..8 records; unwind assertions; header/trailer buffers tested at exact format length.\
**Negative witness:** offset near MAX + length wraps into valid looking offset.\
**Requirement links:** D-UDIF-01, D-UDIF-04.\
**Evidence:** Recorded Kani, property, and fuzz runs pass within the stated bounds. [D-V01 evidence](../../crates/formats/dmgez-udif/verification/evidence/D-V01.json)

### D-V02 — UDRO/UDZO
**Target:** dmgez-udif / run validation, logical map.\
**Property:** Each supported map resolves exactly one data or declared zero source per admitted sector; physical compressed spans stay in data fork; run order, comments, terminal rules and checked lengths agree with profile.\
**Assumptions and initial bounds:** Only fixture-qualified UDIF profiles; finite 0..8 runs and 0..16 sectors; zero/ignore semantics must be established by independent ledger, not an assumption that they are interchangeable.\
**Negative witness:** gap or overlap hidden by sorted index; ignore run treated as stored zero bytes.\
**Requirement links:** D-UDIF-02, D-UDIF-03, D-UDIF-04.\
**Evidence:** Recorded Kani, property, differential, and native runs pass within the stated bounds. The crate verification notes document the stopped experimental physical-sort proof. [D-V02 evidence](../../crates/formats/dmgez-udif/verification/evidence/D-V02.json)

### D-V03 — UDRO/UDZO
**Target:** dmgez-udif / checksum accumulation and finalization.\
**Property:** Stream segmentation does not alter CRC in a single qualified domain; stored digests cover the precise required bytes; a checksum mismatch is never accepted as verified.\
**Assumptions and initial bounds:** CRC library contract tested with independent known vectors; compare all qualified checksum domains and zero/ignore cases to native fixtures.\
**Negative witness:** valid table CRC but wrong data-fork checksum accepted.\
**Requirement links:** D-UDIF-02, D-UDIF-03.\
**Evidence:** Recorded property, differential, and native runs pass within the stated bounds. [D-V03 evidence](../../crates/formats/dmgez-udif/verification/evidence/D-V03.json)

### D-V04 — UDIF reader
**Target:** dmgez-udif / indexed reads and bounded decoder.\
**Property:** An admitted read returns exact bytes or an error. Decoded lengths and capacity are checked before output is accepted. Unknown runs never yield synthetic zeros. Concurrent reads remain exact through eviction, with bounded decoded capacity and per-image isolation.\
**Assumptions and initial bounds:** Qualified codec contracts, small runs <=8 and range partitions, independent arbitrary byte streams, and hostile lengths. The cache model uses two readers, three 512-byte zlib runs, and a two-entry cache.\
**Negative witness:** last partial read crosses block boundary and returns data from adjacent block.\
**Requirement links:** D-UDIF-04.\
**Evidence:** Kani, property, fuzz, and differential runs pass within the stated bounds. Loom explores two overlapping reads across three zlib runs with a two-entry cache and compares both results to the raw fixture. [D-V04 evidence](../../crates/formats/dmgez-udif/verification/evidence/D-V04.json)

### D-V05 — builder
**Target:** dmgez-builder / manifest sealing and metadata.\
**Property:** Accepted manifest has unique normalized sibling names, no escape, correct link kinds and stable generated identities; strict packaging rejects any required unavailable/unsupported metadata rather than guessing.\
**Assumptions and initial bounds:** Filesystem owns canonical equality; host snapshot/identity contracts not absolute concurrency guarantees; generated trees up to 12 nodes plus larger randomized/native cases.\
**Negative witness:** two Unicode equivalent paths silently overwrite; background alias stale CNID.\
**Requirement links:** D-BUILD-02, D-BUILD-03, D-BUILD-04, D-FINDER-01.\
**Evidence:** property, differential, native; status planned.

### D-V06 — builder publication
**Target:** dmgez-builder / stage and commit state machine.\
**Property:** Only a complete validated staged image can be published; any earlier failure leaves previous target intact or absent; no source or unrelated file overwritten; stream failure is reported invalid.\
**Assumptions and initial bounds:** Atomic no-clobber/replace contract of supported host FS; model finite two writers, one cancel, each stage and crash; durability separate from atomic visibility.\
**Negative witness:** ENOSPC after stage; competing builder wins no-clobber race.\
**Requirement links:** D-BUILD-05.\
**Evidence:** tlc, fault, integration; status planned.

### D-V07 — parallel builder
**Target:** dmgez-udif / admission credits, ordered writer.\
**Property:** Each admitted run owns one bounded credit reservation, written at most once in ordinal order; failure cancels and releases credits exactly once; no partial success.\
**Assumptions and initial bounds:** TLC <=4 jobs/3 workers, <=4 credits plus fail/cancel/slow head; Loom equivalent small synchronization model/actual instrumented coordinator; fairness for liveness only with runnable workers and responsive sink.\
**Negative witness:** head job slow, later jobs fill queue, consumer blocked waiting on worker-held resource.\
**Requirement links:** D-PERF-01.\
**Evidence:** tlc, loom, shuttle, property, fault; status planned.

### D-V08 — reproducible build
**Target:** dmgez-udif / pure run scheduling & assembly.\
**Property:** Given same canonical sector source, options, deterministic codec and metadata, every completed worker permutation has identical emitted sequence and same unsigned bytes as serial.\
**Assumptions and initial bounds:** Lean proof for pure ordering/refinement linked via extraction; external encoder determinism is an assumption checked on pinned versions; native cross-host fixtures.\
**Negative witness:** worker completion order controls offsets or plist order.\
**Requirement links:** D-PERF-01, D-PERF-02, D-BUILD-05.\
**Evidence:** lean-linked, property, differential, native; status planned.

### D-V09 — inspection/partition windows
**Target:** dmgez-vfs / bounded source, namespace and checks.\
**Property:** Every subvolume read maps inside authorized immutable image range, selected volume; full verification pass iff all required checks pass; partial/unknown/unsupported not promoted to valid.\
**Assumptions and initial bounds:** Kani full-width arithmetic, nested windows depth <=4, symbolic status combinations; native multi-volume fixtures.\
**Negative witness:** partition offset + relative extent escapes selected partition; unsupported checksum treated as valid.\
**Requirement links:** D-READ-01, D-READ-03, D-READ-04.\
**Evidence:** kani, property, differential; status planned.

### D-V10 — extract
**Target:** dmgez-operations / extraction path admission and publish.\
**Property:** No extraction side effect outside authorized destination root or authorized replace; path validation rejects ambiguous/reserved names; symlinks not followed for writing; per-item failures accurately reported.\
**Assumptions and initial bounds:** Kani pure path grammar bounded 0..8 components; handle-based host adapters, reparse rules and actual races tested per OS; lexical proof not host security proof.\
**Negative witness:** attacker swaps parent directory for symlink between validation and write.\
**Requirement links:** D-READ-05.\
**Evidence:** kani, property, fuzz, fault, integration; status planned.

### D-V11 — GUI project/edit
**Target:** dmgez-config / command model and dmgez-preview / geometry.\
**Property:** Apply/undo/redo restores semantic project state; preview reads actual selected image and does not mount/execute; all coordinate transforms are finite and invertible where defined.\
**Assumptions and initial bounds:** Pure commands <=20 operations reference model; native icon/view differences allowed explicitly.\
**Negative witness:** undo changes path identity or hidden flags; malicious preview triggers open/execute.\
**Requirement links:** D-GUI-01, D-GUI-02, D-GUI-03.\
**Evidence:** property, state-machine, snapshots, integration; status planned.

### D-V12 — signing and release
**Target:** dmgez-signing / release stage machine.\
**Property:** No sign/notary/staple/publication unless authorized stage preconditions and correct artifact identity hold; stale submission/digest or pending/rejected outcome cannot mark ready.\
**Assumptions and initial bounds:** TLC <=2 artifact revisions/2 submissions, ambiguous upload failure, accepted/rejected/cancel; native Apple service treated as external, actual acceptance independent.\
**Negative witness:** reuse prior accepted ticket for modified file; retry ambiguous upload blindly.\
**Requirement links:** D-REL-01, D-REL-02, D-REL-04.\
**Evidence:** tlc, state-machine, fault, native; status planned.

### D-V13 — conversion/edit/cache
**Target:** dmgez-operations + dmgez-cache / overlay and cache key.\
**Property:** No-op overlay preserves all claimed metadata/logical data; untouched entries remain unchanged; cache hit only for exact logical bytes/profile and verified decoded length/digest; cache cannot silently corrupt image.\
**Assumptions and initial bounds:** Reference small image <=16 entries, <=8 runs; cryptographic hash assumption explicit, cache treated hostile; exact comparison in tests.\
**Negative witness:** cached block for same offset but different compression/profile is reused.\
**Requirement links:** D-OPS-01, D-PERF-03.\
**Evidence:** property, fuzz, fault, differential; status planned.

### D-V14 — mount and install
**Target:** dmgez-mount + dmgez-install / session state.\
**Property:** Only managed mount identity may be unmounted; auto-eject requires observed intended fresh copy, verified destination and no outstanding writes under defined completion policy; stale event/old matching app never sufficient.\
**Assumptions and initial bounds:** TLC finite 2 mounts, 2 apps, 2 destination revisions, event loss/reorder/busy/cancel; host completion evidence not guaranteed by notifications; normal OS unmount may refuse.\
**Negative witness:** pre-existing identical app or delayed event causes premature eject.\
**Requirement links:** D-MOUNT-01, D-MOUNT-02, D-MOUNT-03.\
**Evidence:** tlc, state-machine, fault, integration, native; status planned.

### D-V15 — remote and MCP
**Target:** dmgez remote storage adapter + MCP service / approval state.\
**Property:** Every range read uses same admitted resource identity and exact checked offsets; side effects require matching authorization, path scope and current request identity; redirect/changed resource/approval invalidates state.\
**Assumptions and initial bounds:** TLC two resources/revisions and approvals, timeout/redirect; strong validator or full snapshot; actual HTTP and OS contracts tested.\
**Negative witness:** server swaps ETag between trailer and block reads; stale approval permits different output.\
**Requirement links:** D-OPS-03, D-OPS-04.\
**Evidence:** tlc, property, fuzz, fault, integration; status planned.

### D-V16 — UDIF reader/writer and sealed images
**Target:** dmgez-udif / pure checked interval and lookup helpers.\
**Property:** For all representable offsets and validated run-map sizes, Rust range-map lookup refines mathematical partition of logical domain; out-of-domain requests error; concatenated reads equal whole reads.\
**Assumptions and initial bounds:** Lean spec over naturals, extraction of actual safe Rust helpers; codecs/external I/O treated as tested contracts; no unbounded proof of parser or OS.\
**Negative witness:** off-by-one at terminal sector or width conversion.\
**Requirement links:** D-UDIF-01, D-UDIF-03, D-UDIF-04, D-READ-01.\
**Evidence:** lean-linked, property, differential; status planned.

### D-V17 — architecture and release
**Target:** xtask / feature and dependency graph.\
**Property:** Advertised default closure excludes forbidden runtime categories; no cycles, app dependencies or cross-repo reverse dependency; required evidence matches profile, source and toolchain.\
**Assumptions and initial bounds:** All implemented target feature combos and pinned metadata; negative test graph mutations; optional signing transitive exception explicit.\
**Negative witness:** test-only oracle leaks into normal dependency graph.\
**Requirement links:** D-CI-01, D-CI-02, D-CI-03.\
**Evidence:** integration, mutation; status planned.

### D-V18 — advanced/legacy/encryption/writable mounts
**Target:** owning optional format/platform modules.\
**Property:** No unqualified advanced profile accepted for writes; bounds and security contract per envelope/segment/band; encryption auth status honest; writable mounts preserve independent transaction gates.\
**Assumptions and initial bounds:** Separate threat model, legal fixture and per-variant obligation additions required before activation; no claims about unimplemented formats.\
**Negative witness:** unknown mandatory flag ignored, segment path escapes root or invalid tag reported valid.\
**Requirement links:** D-OPS-05.\
**Evidence:** property, fuzz, differential, native, fault; status planned.

## Qualification work packages

- Define immutable pure core APIs and source/feature traceability before implementing proof harnesses. Start with range safety and binary framing, then reference set/map and traversal models.
- Create `tests/support` fault and fixture adapters, `fuzz` targets, Kani harnesses and `verification/models` for the actual registered scopes; preserve failing seeds as regression cases.
- Carry out implementation-linked Lean extraction on the first small eligible pure function before building broader theorem obligations. Failure to extract is not a proof; record alternate backend/evidence and amend the gate by explicit review before activation.
- Before shipping an activated protocol, check finite TLA+/TLC model and replay its actions/failures against production Rust. Native fixtures and actual fault/integration checks remain mandatory.
- Evidence is committed to the CI release report, never to a prefilled pass status. The present architecture pack does not contain a tested Rust implementation.
