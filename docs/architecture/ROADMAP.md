# dmgez release roadmap

These are scope and evidence gates, not dates. Every release claims only the supported profile recorded in its compatibility ledger. HFS+ and APFS versions are independent; HFS+ initial clean-volume build is critical-path, APFS is not. Feature gates introduced only as functionality arrives; no empty switches or stubs marketed as support.

| Release | Scope | Required gates (requirement families) | Nonblocking deferrals |
|---|---|---|---|
| 0.1.0 | `dmg.toml` to a partitionless HFS+ image in single-segment UDRO or UDZO, with dry-run, offline ad hoc signing, verification, and standalone CLI archives | D-UDIF-01..04, initial H-FMT/BUILD/READ profiles, native attach and read-only fsck, independent raw-byte comparison, malformed inputs, cross-host project and archive checks | Finder parity, GUI, APFS |
| 0.2.0 | Expand the project builder to arbitrary content, Mac metadata, basic Finder layout, bounded Rayon default, and progress reporting | D-BUILD-01..05, D-FINDER-01, D-PERF-01, D-CI-01; build on all three hosts and attach on macOS | Advanced layout, complete read CLI |
| 0.3.0 | Practical dmgbuild replacement for modern HFS+ installers; complete applicable root-view settings, multi-res assets, list view, icons, import, performance baselines | D-FINDER-02..03, D-UDIF-05 (ULFO qualified opt-in), compatibility inventory (all applicable settings mapped, test or explicit defer); D-PERF-02 unsigned reproducibility | APFS, sparse, legacy/EULA, deprecated unsupported views; clearly documented differences |
| 0.4.0 | Deep inspection, VFS, verify, extract, partition detection, native reference comparisons, HFSX read where qualified | D-READ-01..05, D-UDIF-06 (signed metadata/multi-table qualified as applicable); malformed images and safe extraction | No OS mount required |
| 0.5.0 | GUI builder, actual DMG explorer, technical inspector, shared headless preview, project round-trip | D-GUI-01..03; GUI works without mount drivers; native Finder oracle and keyboard/accessibility | OS drag-out integration later |
| 0.6.0 | Opt-in native Developer ID signing and notarization, advanced release gates, semantic diff and lint | D-REL-01..04, protected native tests, explicit trust vs structural states | Cross-platform Gatekeeper evaluation impossible; report unavailable |
| 0.7.0 | Edit/rebuild, format convert, optimize, analyze, incremental block cache | D-OPS-01..02, D-PERF-03; source immutable, metadata loss policy, signature invalidation | Unsafe in-place compressed edits |
| 0.8.0 | Virtual install and drag-out, optional host mounting, managed install monitoring & safe ejection | D-MOUNT-01..03; separate per-OS backend qualification, no forced eject | Writable mount, no automatic DMG code execution |
| 0.9.0 | MCP, remote range-backed image inspect, ULMO/UDRW and partitioned layouts as qualified | D-OPS-03..04; gated remote/MCP permissions and cross-platform tests; modern codec matrix | Any unqualified variant remains deferred |
| 1.0.0 | Stabilize advertised capabilities, schema and Rust API, support matrix, releases/CI, Dockhand integration | D-CI-02..03; all non-deferred requirements for advertised profile pass; review limits and evidence | APFS, legacy, encryption, ASIF/sparse, segmented, writable mounts, deprecated license resources remain optional stretch |

**First major payoff is 0.3.0.** Once it builds actual signed app installers on all three host OSes, subsequent services are independent opt-in releases. First gate is raw UDRO then UDZO, not a polished GUI. HFS+ build/release qualification may occur concurrently with UDIF; native `newfs_hfs` fixtures allow UDIF development before HFS+ writer. HFS+ 0.1 read-only subset supports verification tests but need not link into default dmgez. Start APFS container research in parallel only if resources permit. Never block HFS+ releases on Charles integration.

## Milestone acceptance procedure

The [v0.1.0 release profile](../features/RELEASE.md#v010-release-profile) records the project workflow and its current evidence.

1. Open `docs/reference/requirements.json`, select all relevant requirement IDs and mandatory acceptance cases in `test-catalog.json`. Check owning spec, version, evidence IDs and prerequisites.
2. Implement only current scope, update capability graph and ledger; test negative cases early. Run docs validator, unit/property/fuzz smoke, dependency graph and cross-target feature jobs.
3. Execute native oracle jobs on licensed disposable environments, record OS build/tool flags and input/output digests, collect structured reports. A skipped oracle job is not a pass; restrict release profiles if evidence unavailable. Native external tools are not runtime builder dependencies.
4. Run pinned independent differential implementations for overlapping support. Resolve deviations or mark explicit unsupported scope; never weaken tests to pass. If security, metadata loss or structural corruption unresolved, no release for affected capability.
5. Publish docs/capability matrix, binaries and crates with dependency notices and reproducible build evidence. Confirm package names free. Version schema migrations and stable diagnostics; document known differences from dmgbuild and unsupported OS/format profiles.

Stretch features `legacy` (obsolete formats incl ADC/UDBZ/NDIF/DC42), `license` (deprecated EULA), `encryption`, `sparse`, `asif`, `segmented`, `mount-write`, `apfs-read`, `apfs-build` have explicit separate research/security/compatibility gates (D-OPS-05). Legacy formats, advanced readers, GUI, and remote access stay off by default. Signing follows the per-host release profile. FAT32/exFAT/NTFS logical filesystem creation and recovery/carving are not planned. A separate standalone APFS project can be released independently with its own roadmap.

## Verification integration

The local [verification plan](VERIFICATION.md) and `verification/obligations.json` define mandatory profile-specific verification gates in addition to the original acceptance tests. Apply ecosystem standard EVS-1 (copy `shared/VERIFICATION.md` as `docs/architecture/ECOSYSTEM_VERIFICATION.md`). No required proof, model check, native test or critical fault gate can be substituted by coverage or a self-generated round trip. `cargo-nextest`, property tests, fuzz/corpus replay, coverage and deterministic architecture checks are required; formal methods activate at the milestones in the obligation registry.
