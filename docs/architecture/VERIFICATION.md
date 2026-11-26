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

The clean archive candidate `017b843334eb2e52eddd88347bba6571198bf12a` also passed all 18 records on macOS 14.7.2 arm64. It ran 16 Kani harnesses with Kani 0.68.0 and CBMC 6.11.0, 116 UDIF tests with 4,096 property cases, four Loom scenarios, and 20,000 AddressSanitizer inputs from the eight checked-in seeds. The crash directory was empty. Both Rust reference decoders matched all four comparison images. The exact archived CLI passed the native read-only HFS+ attachment and `fsck_hfs` check. The validator rejected missing evidence, a stale source digest, a dirty checkout, wrong features, timed-out and unsupported records, a mismatched transcript digest, and an unreached cover property.

The candidate source archive has SHA-256 `b438eba3dec709c4e5a62396a0fcb1e28908b13217f88e662cdfea4a15dcf994`. The bundle and validation transcripts are in `target/verification-evidence/v0.1.0-verification/017b843-release-replay/`.

| Artifact | SHA-256 |
| --- | --- |
| `bundle.json` | `f77b409eb09f90eac309a66daa71bb403217129ae810a1d5f8b81f4995ecc931` |
| `validation.log` | `c468533ba6f92b0b52da5f48328570da9ee4eade373f88980373cb8ace067cd4` |
| `validation-rejections.log` | `10427649923907cdbd9009f6ecfc5f7b6d5cf24860078cbd0461f12ac7084dd8` |

The refreshed archive revision `ec0f4bd122f60de956de93a1fabce09dd895f4b9` passed all 18 release-evidence records with its own clean source digest `0f9daff41238204827591658c2157658f89d8091ff26de5a950f6be840197828`. The replay covered 16 Kani harnesses, 116 UDIF tests, four Loom cases, and 20,000 sanitizer inputs with an empty crash directory. Both independent reference readers matched all four comparisons, and the checksum-pinned hosted macOS CLI passed read-only HFS+ attachment and `fsck_hfs`. The validator rejected the same eight invalid bundles and checkouts. The `bundle.json` SHA-256 is `19a820a7ec249837c97081e1f15f1de82ad90b921cdd315d2f080abda2ae2414`. The bundle and transcripts are in `target/verification-evidence/v0.1.0-verification/ec0f4bd-release-replay/`.

The held-stage revision `c76fcf18832a26a978089faa16fff8d7d43da605` passed all 18 records against clean source archive SHA-256 `19595560d2295e1a4030e28d7e2e5a47328a6e367364286539827a6093e47029`. The local replay ran 16 Kani harnesses with 0.68.0, 116 UDIF tests with 4,096 property cases, four Loom cases, and 20,000 AddressSanitizer inputs from nine seeds with an empty crash directory. The locally composed macOS CLI passed read-only HFS+ attachment and `fsck_hfs`, and both pinned independent reference readers matched all four comparisons. The validator rejected all eight negative trials. The bundle and transcripts are in `target/verification-evidence/v0.1.0-verification/c76fcf1-release-replay/`. The `bundle.json` SHA-256 is `89cbf509bb7e1a4b0907698495e8e5707cf9865d72507ff5012a859baed431a8`.

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

All 334 file rows in that report match tracked files from the candidate. Their counters sum to the reported totals. The full-workspace feature set includes capabilities scheduled after v0.1.0. The following totals describe the instrumented packages within that run.

| Package | Lines covered | Instrumented lines | Coverage |
| --- | ---: | ---: | ---: |
| `dmgez-builder` | 4,409 | 5,450 | 80.90% |
| `dmgez-config` | 1,595 | 1,644 | 97.02% |
| `dmgez-core` | 401 | 468 | 85.68% |
| `dmgez-udif` | 3,012 | 3,123 | 96.45% |
| `dmgez-signing` | 788 | 998 | 78.96% |
| `dmgez-runtime` | 561 | 1,049 | 53.48% |
| `dmgez-cli` | 883 | 1,236 | 71.44% |

The hosted workspace contains no instrumented HFS+ adapter rows. The separate composed CLI workflow exercises that adapter on each host. The workspace row-level inventory is in `target/verification-evidence/v0.1.0-verification/ffb76e4-hosted-coverage-scope.json` with SHA-256 `8a537b7e54bcd5eeaf6c90980d1e48e2eb3374980ed0aed66c33aa3d36f82b5a`.

The composed smoke baseline at dmgez `2950125` used the HFS+ sibling checkout at `a1b870f`. An optimized arm64 macOS build used the default features plus `inspect` and nightly `2026-11-04` with `-C instrument-coverage -Z coverage-options=branch`. The instrumented executable reproduced all four expected signed and unsigned UDRO and UDZO digests. 7-Zip, strict Apple signature verification, read-only HFS+ checking and mounts, and the offline archive smoke with fixture notices all passed. The merge collected 124 process profiles across 225 tracked source rows.

| Package | Lines covered | Branches covered |
| --- | ---: | ---: |
| `dmgez-cli` | 469 / 1,085 | 28 / 132 |
| `dmgez-runtime` | 582 / 1,260 | 67 / 170 |
| `dmgez-builder` | 1,434 / 4,567 | 123 / 781 |
| `dmgez-udif` | 1,503 / 2,031 | 166 / 346 |
| `dmgez-signing` | 544 / 882 | 46 / 148 |
| `dmgez-hfsplus` | 535 / 852 | 25 / 78 |
| `hfsplus-builder` | 1,258 / 1,980 | 83 / 174 |

These counts reflect the end-to-end project and archive smokes. The instrumented binary includes compiled optional paths the smokes leave unexercised, such as Finder backgrounds, and the run excludes standalone unit, fault, and property tests. D-VER-01 still requires the release-profile coverage review to account for those paths and the corresponding focused tests. The revision-bound inventory is in `target/verification-evidence/v0.1.0-verification/2950125-composed-project-coverage.json` with SHA-256 `8cd4d1909037640210c144e68b229e4c0d3cd097adb39fd51e67f462fdc0dbc3`.

The composed CLI fault test builds a verified unsigned UDRO project, damages one payload byte, and invokes the default ad hoc signing command. It checks exit status 2, the integrity diagnostic, an empty stdout, preservation of the damaged input, and absence of a signed output or private stage. The pinned Rust build with HFS+ `a1b870f` passed 18 project CLI tests and two binary unit tests on arm64 macOS. [Project release tests](../../apps/dmgez-cli/tests/project_build/release.rs)

At dmgez `017b843` and HFS+ `a1b870f`, the optimized composed CLI with default features plus `inspect` passed its 12-test project suite, the native UDRO and UDZO smokes, and the offline archive smoke. The measurement used separate process profiles from that same compiled CLI. The smoke generated 29 CLI profiles and the project tests generated 21 more. The comparison excluded 103 profiles from other binaries. The filtered merge had no LLVM warnings. A trial merge that included those binaries emitted ten function mismatches and was excluded from qualification.

| Package | Lines covered by smokes → combined | Branches covered by smokes → combined |
| --- | ---: | ---: |
| `dmgez-cli` | 469 → 516 / 1,085 | 28 → 33 / 132 |
| `dmgez-runtime` | 582 → 622 / 1,260 | 67 → 76 / 170 |
| `dmgez-builder` | 1,434 → 1,532 / 4,567 | 123 → 149 / 781 |
| `dmgez-config` | 397 → 474 / 809 | 48 → 61 / 158 |
| `dmgez-udif` | 1,503 → 1,512 / 2,031 | 166 → 168 / 346 |
| `dmgez-signing` | 544 → 549 / 882 | 46 → 47 / 148 |
| `dmgez-hfsplus` | 535 → 541 / 852 | 25 → 27 / 78 |
| `hfsplus-builder` | 1,258 → 1,258 / 1,980 | 83 → 83 / 174 |

All mapped file denominators matched across the two runs. The inventory is in `target/verification-evidence/v0.1.0-verification/017b843-composed-cli-only-coverage.json` with SHA-256 `e48338f980e9ede2d1b19e64d54662c01961f36757f59a0d3cbc8fcb1e939452`. The coverage review for lower-level fault and property tests remains open under D-VER-01.

The lower-level replay used dmgez `1186638` and HFS+ `a1b870f` on arm64 macOS. The three builder execution and discovery suites passed 28 tests. They cover short writes, cancellation, changed sources, verifier refusal, private-stage cleanup, and rejection of mismatched raw and UDIF lengths. The staging suite passed 11 tests with Rust 1.99.1, including preservation of incumbent bytes, late destination creation, and refusal after stage or parent directory substitution.

The HFS+ builder's unit and integration suites passed 42 tests. The sealed-plan suite checks every partial read in its bounded fixture against the materialized bytes and preserves source and sink faults and cancellation. Three ignored fixture and oracle tests ran separately. The canonical witness matched SHA-256 `63e2f4d50df05f59a1e6dbe63c77e5d75b10e1e2780bae068af3eec4faeadba6` in both manifest orders. 7-Zip matched its Unicode file, symlink, and link-inode payloads and read a separate constructed volume. Apple's read-only filesystem check passed after a raw-device permission warning. The HFSX emitter produced the fixture for independent checking.

The builder and HFS+ exports used nightly `2026-11-04` and retained coverage by test executable. The maps have different denominators, so each row describes its own test scope. For example, builder execution reached 1,167 of 4,530 source lines and 132 of 778 branches. The HFS+ sealed-plan tests reached 172 of 987 source lines and 16 of 100 branches. The inventories are under `target/verification-evidence/v0.1.0-verification/`.

| Inventory | SHA-256 |
| --- | --- |
| `1186638-builder-fault-suites.json` | `1af87d771e0912ce6fb928891eae79f0c467276f19e678a4be9081a71126a383` |
| `1186638-staging-suites.json` | `63e144e1c468a7cfd69a6f54151529fa2d7cf3f720daa25f962ebd866d4fe1a5` |
| `a1b870f-hfsplus-layout-suites.json` | `1d516e4f535b091c2164d6b344bd94e861b7c6db01041edd632ec38fa0c42ab8` |

The detailed UDIF replay at `9b68c63` reproduced the per-file totals above. The exported source counters showed no rejected outcome at the empty-table check and the reversed-fragment comparison. The block-map decoder rejects an empty table before normalization, and the positioned-window Kani harness tests arbitrary fragment ordering. The LLVM summary also reports one uncovered cache branch even though the merged source-location counters show both outcomes for every exported cache condition. The per-file summaries and condition counters are in `target/verification-evidence/v0.1.0-verification/9b68c63-branch-conditions.json` with SHA-256 `56fc87dfa490f2b3fe9510012353185ac7931469685311e8dca68b28415ee85d`.

The positioned read-window regression at `e89d1e0` checks a reversed fragment, a fragment starting before the request, a buffer one byte too short, and exact-fit and empty windows at the end of the address space. All 19 reader-only unit tests and strict Clippy passed. The matching nightly instrumentation passed 116 all-feature tests and measured `index.rs` at 98.48% lines and 90.38% branches (47 of 52). The cache branch discrepancy and workspace coverage review remain open.

The cache regression now exercises oversized rejection and successful recovery through one decoder call site. LLVM recorded one true and one false outcome for that concrete capacity-check instance. The file export recorded both outcomes for each of its three instrumented conditions, including entry-count eviction and retained-capacity eviction. The file summary still reports five of six covered branches. The all-feature suite passed 116 tests. The baseline instantiation annotations are in `target/verification-evidence/v0.1.0-verification/7a361b6-cache-branch-annotations.log`.

The `017b843` archive candidate reproduced the UDIF branch results on macOS arm64 with nightly `2026-11-04` and 4,096 property cases. `index.rs` covered 47 of 52 branches, `reader.rs` covered 23 of 24, and `trailer.rs` covered 22 of 24. LLVM's cache summary reported five of six branches. The cache export recorded 28 true and 3,736 false outcomes for entry-count eviction, two true and 3,734 false outcomes for capacity eviction, and one true and 3,726 false outcomes for oversized decode rejection across the instrumented call sites. The branch inventory is `target/verification-evidence/v0.1.0-verification/017b843-release-replay/udif-branch-coverage.json` with SHA-256 `a2dd836e0997aecfececb3dc680cd1f9528eb1fa73f9662790206048ee61b61f`.

On the same revision, hosted all-feature workspace coverage was 27,762 of 32,776 lines (84.70%) and 41,613 of 50,401 regions (82.56%). All 334 reported file rows match tracked files, and their counters sum to the job's totals. The Linux job compiled the locked all-feature workspace with Rust 1.98.0. The hosted workspace figure and the separate composed HFS+ measurements remain inputs to the open D-VER-01 coverage review. The hosted inventory is `target/verification-evidence/v0.1.0-verification/017b843-release-replay/hosted-workspace-coverage.json` with SHA-256 `b8755e57ca74b3eff3b947a9be21585d694b79067ec69b3399dd57e25fa32798`. [Hosted verification](https://github.com/pomologie/dmgez/actions/runs/45795137044)

The hosted all-feature workspace job at refreshed archive revision `ec0f4bd` reproduced those line and region totals and passed the minimum-Rust check. This hosted measurement uses the generic workspace, and the artifact publication branches have a separate composed HFS+ measurement below. [Refreshed coverage and minimum-Rust job](https://github.com/pomologie/dmgez/actions/runs/45806613540/job/139863436681)

The hosted coverage job uploads `workspace-coverage` with the LLVM per-file report, its text summary, the checkout revision, tool versions, and checksums. The same test execution supplies both reports. This artifact lets reviewers inspect file and source-region coverage for the exact run. The separate composed measures below cover the HFS+ release workflow. [Hosted workflow](../../.github/workflows/ci.yml)

The composed runtime at dmgez `641e14e` and HFS+ `a1b870f` passed 14 release tests. With the four fault cases from `48b524a`, it passed 18 tests. Both runs used nightly `2026-11-04` branch instrumentation on arm64 macOS with `project-release,signing-rust`. The new cases reject source-length changes before staging, reject empty, oversized or short input during identity calculation, detect distinct Unix inodes at the same length, and refuse ad hoc signing when the portable backend is unavailable.

| Runtime module | Baseline lines | Tested lines | Baseline branches | Tested branches |
| --- | ---: | ---: | ---: | ---: |
| `project_release/artifact.rs` | 82 / 87 | 85 / 87 | 16 / 26 | 20 / 26 |
| `project_release/settings.rs` | 163 / 187 | 163 / 187 | 43 / 44 | 44 / 44 |

The matching runtime unit suite passed 21 tests on stable Rust, and strict Clippy passed for the same composed features. The comparison and source-location counters are in `target/verification-evidence/v0.1.0-verification/641e14e-release-guards/release-guards-coverage.json` with SHA-256 `9b2004818abf88b708c1d38acd39d7b9356b595c623ca4192d24c1fe59a9c591`. Publication branch coverage remains part of the D-VER-01 review.

The local arm64 macOS replay of the hosted test command, `python3 -B tools/compose_hfsplus_cli.py --hfsplus ../hfsplus --release --features inspect --test`, passed the 12 project CLI tests and all 21 composed runtime tests. The hosted project matrix runs that command on each target.

The follow-up at `09400d8` added witnesses for length and timestamp changes on the same held file and for exact hashing across a buffer boundary. With those tests applied, all 20 filtered runtime release tests and strict Clippy passed. Artifact coverage reached 21 of 26 branches in the same instrumented profile. The report is `target/verification-evidence/v0.1.0-verification/09400d8-release-identity/runtime-identity-coverage.json` with SHA-256 `38f3551b0a8053cdffce0e33e485a39005ddc7617042dfe083c29e2f4d637a47`.

The artifact opener now validates the held regular file against the named metadata sampled before and after opening. The composed runtime at `fe4f928` with this change and HFS+ `a1b870f` passed 21 filtered runtime release tests and strict Clippy on arm64 macOS. The artifact module covered 90 of 91 lines and 23 of 26 branches. The unmatched outcomes were a short successful copy, differing Unix device IDs, and a nonregular comparison operand with matching Unix identity. The bounded `CapturedFile` reader returns exact chunks before the admitted end and reports short reads as errors. The composed CLI reproduced all four signed and unsigned fixture digests, passed the Apple and 7-Zip comparisons, and passed the offline archive smoke with fixture notices. The coverage report is `target/verification-evidence/v0.1.0-verification/fe4f928-artifact-binding/runtime-identity-coverage.json` with SHA-256 `f92a77d019b9bbf4dd4103d7498b7a177d743307d8b6f206decc3b6d4ee8163a`.

At archive revision `ec0f4bd`, hosted Nextest passed 892 tests on macOS, 772 on Linux and 688 on Windows, with eight, six and six skipped respectively. The clean checkout passed the architecture checker and 12 `xtask` tests, including the negative CLI dependency graph test. The source checker rejected a disposable 501-line source file. [Hosted workspace verification](https://github.com/pomologie/dmgez/actions/runs/45806613540)

The optimized, instrumented CLI at the same revision and HFS+ `a1b870f` reproduced the four fixture hashes and passed the offline archive smoke. The same build passed 12 project CLI tests and 24 runtime tests. The CLI map contains 29 smoke and 21 project-test subprocess profiles. The runtime map uses its own test executable and profile. The build isolated 110 build-time profiles. The report excludes vendor code and test modules, verifies the remaining source bytes against the pinned checkouts, and retains branch counters by source location.

| Source scope | CLI lines | CLI branches | Runtime test lines | Runtime test branches |
| --- | ---: | ---: | ---: | ---: |
| `dmgez-runtime` | 626 / 1,264 | 76 / 170 | 469 / 1,264 | 87 / 104 |
| `project_release/artifact.rs` | 84 / 91 | 14 / 26 | 90 / 91 | 23 / 26 |
| `dmgez-builder` | 1,532 / 4,567 | 149 / 781 | 0 / 4,567 | 0 / 775 |
| `dmgez-hfsplus` | 541 / 852 | 27 / 78 | 0 / 852 | 0 / 78 |
| `hfsplus-builder` | 1,258 / 1,980 | 83 / 174 | 0 / 1,980 | 0 / 174 |

The coverage inventory has SHA-256 `de5bec140546eaf3ec663de8f4f0f9bf3996130fce882e746bbb1736e4a52e3f` and is stored alongside the architecture and hosted test summaries under `target/verification-evidence/v0.1.0-verification/ec0f4bd-composed-coverage/`. The builder and core sources and the lockfile match the earlier `1186638` fault-suite inputs. The HFS+ builder still uses the separately tested `a1b870f` revision. The per-executable counts and remaining critical branch outcomes are inputs to the open D-VER-01 review.

The captured-source regression exercises two independent reader cursors over one held file and checks exact completion at its admitted extent. It also confirms that truncation returns `SourceError::Changed` through `io::copy` without appending another byte. Both cases passed with the source feature alone. The core all-feature suite passed 19 tests and strict Clippy. [Captured-source tests](../../crates/foundation/dmgez-core/tests/captured_source.rs)

The builder execution suite covers three release staging refusals. A stream plan creates no output. Cancellation at `VerifyingOutput` skips the verifier, removes the private stage and preserves the incumbent. Source truncation during successful verification returns `ManifestError::SourceChanged` and also removes the private stage without replacing the incumbent. All 16 execution tests passed on arm64 macOS. With default builder features, the nightly coverage run recorded 48 of 54 lines for `release_stage.rs`. Both outcomes of the two admission conditions appear across generic instantiations. LLVM's file summary reports 2 of 4 branch outcomes, so the source-span observations are recorded separately from the reported branch percentage. The report is `target/verification-evidence/v0.1.0-verification/5848a2f-builder-release-faults/release-stage-coverage.json` with SHA-256 `0e47740198b3922dc253ffc7b60649ba7dc38fca2a797b9ad9a6d75abef0f83b`. D-VER-01 remains open. [Release staging tests](../../crates/build/dmgez-builder/tests/execution/release_faults.rs)

The publication regression distinguishes three failures through the composed runtime. A different valid UDRO image at the admitted length fails identity validation. A corrupt UDZO payload with a matching SHA-256 fails its UDIF checksum check. An occupied destination returns an explicit commit conflict and retains both its original bytes and the source. Each failure removes the private stage. The 23 filtered release tests and strict Clippy passed on arm64 macOS with dmgez `98f4442` plus these tests and HFS+ `a1b870f`. In a disposable composition, the same three tests each failed when its respective production guard was removed or changed to permit replacement. The exact mutations and transcript digests are in `target/verification-evidence/v0.1.0-verification/98f4442-publication-integrity/summary.json` with SHA-256 `1ba8493c78e720a0f0d09ebf144fc3c9c207ca50440f6512686909f4db494b6c`. [Publication tests](../../crates/runtime/dmgez-runtime/src/project_release/artifact/tests.rs)

The `project_manifest` fuzz target exercises `Project::parse` with seeds from the offline example and the cross-platform CI project. For accepted input it checks fixed capacity, sector alignment, the free-space bound, and unique non-root destinations. The manifest and UDIF targets each completed 20,000 inputs with seed `20261126` under AddressSanitizer on arm64 macOS. These were bounded smoke runs with a 64 KiB input cap. The log digests are `cac44fa7119d7207c0c67cb9726d84f4d48243288ac3944c81dba6267398af20` for `project_manifest.log` and `38a79708a2408d735f2d1b5573390c807069ec46295518eb632cfe7554f3c153` for `udif.log` under `target/verification-evidence/v0.1.0-verification/c4655a8-project-manifest-fuzz/`. [Manifest fuzz target](../../fuzz/fuzz_targets/project_manifest.rs) · [Hosted fuzz job](../../.github/workflows/ci.yml)

The composition at dmgez `26ba0d1` and HFS+ `a1b870f` ran 360 unit and integration tests and both native project fixtures on arm64 macOS. One nightly `2026-11-04` debug build instrumented the composed CLI defaults plus `inspect`, using 4,096 property cases with seed `20261126`. The merged map combines 211 test and 20 smoke profiles. The two native fixture runs reproduced all four expected DMG hashes and passed signed dry-run, occupied-destination, Apple, and 7-Zip checks. The architecture and CLI dependency profile checks also passed. [Project fixture](../../tests/compatibility/project-build/README.md)

The table excludes 13 standalone `tests.rs` files from the source inventory. Inline tests in mixed source files remain mapped. The two HFS+ rows reflect their use by the composed CLI only. The HFS+ crate tests have separate evidence.

| Source scope | Lines covered | Branch outcomes covered |
| --- | ---: | ---: |
| `dmgez-cli` | 799 / 1,092 | 70 / 132 |
| `dmgez-runtime` | 926 / 1,264 | 122 / 182 |
| `dmgez-builder` | 4,151 / 4,738 | 551 / 786 |
| `dmgez-config` | 783 / 809 | 126 / 158 |
| `dmgez-core` | 266 / 312 | 27 / 48 |
| `dmgez-signing` | 726 / 882 | 99 / 148 |
| `dmgez-udif` | 1,902 / 2,040 | 309 / 376 |
| `dmgez-hfsplus` | 550 / 852 | 27 / 78 |
| `hfsplus-builder` | 1,259 / 1,980 | 83 / 174 |

The seven directly tested packages covered 9,553 of 11,137 lines (85.78%) in this profile. The workspace-wide audit and critical-file review remain open. The combined map still reports 23 of 26 branches for `project_release/artifact.rs`, with separate mutation evidence for the three publication guards above, and five of six for `reader/cache.rs`, with source-condition counters recorded above. The report binds 238 tracked source files by SHA-256 and excludes 210 build-time profiles. The SHA-256 of `coverage.json` is `975987498b3924ce9a2edc6c9a54b197ae64f1cf26e641ae1687dfbfad97ecf1`. The report, run logs, and `reproduce.py` are in `target/verification-evidence/v0.1.0-verification/26ba0d1-combined-release-coverage/`.

The HFS+ release dependency at `a1b870f` also passed a separate merged run of all 42 builder tests, with three opt-in tests left out of the map. The map covers 1,921 of 2,122 lines and 160 of 182 branches in 14 builder source files. `allocation.rs` and `plan.rs` each cover all 12 reported branch outcomes. Kani passed all five allocation and placement harnesses. The native canonical-volume replay passed read-only `fsck_hfs`, and the mounted volume's reported total and free allocation counts matched its header. These measurements supplement the composed CLI coverage above. [HFS+ builder verification](https://github.com/pomologie/hfsplus/blob/main/crates/filesystem/hfsplus-builder/verification/README.md)

The HFS+ sealed-plan suite reached all six streaming branch outcomes with cancellation before and after a later input chunk and exact reader retry and EOF checks. All 44 builder tests passed with the new tests. The streaming source is byte-identical to the `a1b870f` release dependency. At the archived dmgez revision `c76fcf1`, the hosted all-feature workspace covered 27,766 of 32,776 lines (84.71%) and passed its Rust 1.98.0 check. The workspace audit reported unmaintained dependencies in the desktop and MCP dependency trees. The separate three-target standalone CLI audit passed. D-VER-01 remains open for the workspace and remaining critical-file coverage review. [HFS+ builder verification](https://github.com/pomologie/hfsplus/blob/main/crates/filesystem/hfsplus-builder/verification/README.md) · [Workspace coverage](https://github.com/pomologie/dmgez/actions/runs/45817074693/job/139899467169) · [Standalone CLI audit](https://github.com/pomologie/dmgez/actions/runs/45817074710/job/139904407460)

The POSIX source-discovery suite now checks the final named binding of a recursively opened root by moving its ancestor while the root descriptor is held and replacing the original path. It also verifies that excluded names count against the visit limit even when they contribute no image entries. All 13 manifest-discovery tests and strict Clippy passed on arm64 macOS. Each new test failed when its corresponding check was bypassed in a disposable checkout. The exact edits and test log digests are in `target/verification-evidence/v0.1.0-verification/a3f12b5-discovery-guards/summary.json` with SHA-256 `ff3382499bad9dbd96a0c8427300277070c6db3fc9f0cd8e746c14b89c37e52c`. [Manifest discovery tests](../../crates/build/dmgez-builder/tests/manifest_discovery.rs)

The release composer records the unsigned SHA-256 through the writer-verified stage's held reader and checks its size against the writer report. The next verification step reopens the stage name and checks it against that identity before signing. The Unix replacement fixture confirms the held reader still identifies the verified file and the replacement has a different identity. All 12 optimized project CLI and 28 runtime tests passed on arm64 macOS with HFS+ `a1b870f`, as did strict Clippy and the architecture check. The local native, 7-Zip and offline archive smoke runs reproduced the four established DMG hashes. Logs are under `target/verification-evidence/v0.1.0-verification/c3e5751-held-unsigned-identity/`. The composed test log has SHA-256 `ebb3e1adbc31234946bca1575403cd71e90f87dbc112fc45558e01fd1a6bba42`. [Held stage tests](../../crates/runtime/dmgez-runtime/src/project_release/artifact/tests.rs)

Portable signature verification now checks in regression tests that a changed image body is rejected before the signing policy callback runs. On Unix, a second test checks that verification rejects a byte-identical replacement at the original path after the policy callback. All nine portable signing tests and strict Clippy passed on arm64 macOS. Each test caught the corresponding bypass in a disposable checkout. The exact changes and log digests are in `target/verification-evidence/v0.1.0-verification/ed06f11-portable-verify/summary.json` with SHA-256 `6d998677a7e2b627ab797cff7659539c58bc71be3d95d494a4324597e62b3191`. [Portable signing tests](../../crates/security/dmgez-signing/src/portable/tests.rs)

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
