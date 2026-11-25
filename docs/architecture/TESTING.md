# dmgez test architecture and release qualification

## Test levels

| Layer | Required evidence |
|---|---|
| Unit | Checked binary encoders/decoders, offset units, CRC domains, schema validation, name/collision policy, Finder property mapping, state reducers |
| Property | Run/sector coverage, no overlap/overflow, exact decode lengths, range reads, manifest deterministic ordering, budgets, serializer determinism, no source mutation |
| Differential | Read reference-generated inputs and reference reads of ours for supported profiles; compare decoded disk bytes and semantic metadata, not arbitrary allocator bytes; sources S01-S06 |
| Native interoperability | Build on macOS/Linux/Windows, attach/verify on macOS, read-only fsck, actual mounted file/fork/attr/mode/link comparison and Finder layout behavior |
| End-to-end | CLI TOML/build/inspect/extract/diff/edit, GUI real image and project round-trip, optional mount/install and signing workflows, MCP capability/approval tests |
| Adversarial | Malformed input ranges, decompression bombs, XML/DS Store/ICNS parsers, hostile filenames/control sequences, symlink extraction race, credential/remote injection, cancellation/failure recovery |
| Performance | Controlled benchmark corpus, throughput/size/memory/temp disk, regression alerts; no claims before results |

## Fixture layout and oracle protocol

```text
tests/fixtures/
  manifests/<id>.json          # source generator, seed, profile, OS build, tool argv, checksums
  source/<id>/                 # synthetic tree or reproducible generator instructions
  images/<id>.*                # tiny redistributable sample images; large generated on demand
  expected/<id>.json           # independent decoded ranges + logical tree + raw records as needed
  malformed/<id>.json          # explicit mutation recipe and expected failure category
```

Fixture manifest records `id`, source provenance/license, native OS and exact tool versions/argv, filesystem/partition/container/codec/profile, input and output digests, expected tree bytes/metadata and scope, independent checksums/reference outputs. Synthetic data only; don't commit full macOS system images, third-party app bundles without permission, private signing keys, real user data, or Apple assets. Generate huge images on demand and store seeds/manifests. Use known native fixture for writer comparison, not just same-code readback. `apple-dmg`, `udif`, fstool and hfsplus independent adapter tests only; reference may lack codec coverage, report unsupported. No copy of reference source as implementation. For HFS+ and APFS raw images run their respective repo's native tests and pin tested releases here.

Native harness on licensed macOS targets, first Sequoia and an additional supported release/hardware where available; add exact build/Intel vs Apple Silicon if relevant. Commands are version-qualified, not blind static invocation. Create disposable mount directories and raw images, never pass user disk/device to format/check. Use trusted system executables with argv; attach read-only/no browse for verification, native Finder open only in explicit visual test. Obtain device/mount identity from structured output, fsck read-only on appropriate detached test device; compare tree via native lstat/readlink/xattr/forks, compare signature before/after packaging/extraction, assert Applications link, hidden flags, background alias and window positions. Finally safe detach; don't force other mounts. Native screenshot evidence visual and record-based, tolerances documented; separate actual behavior from serialized settings. Ad hoc signed synthetic bundles for normal tests; protected credentialed Developer ID release only, not on public PR runners. Sign/notarize/staple test exact staged artifact and native Gatekeeper independently. Logs redacted; skipped/unavailable are not passed. Actual Apple commands allowed in tests and explicit optional native mount/sign, never default build.

## CI graph

Run `cargo run --locked -p xtask -- architecture` to check Rust source structure, source size and the resolved CLI dependency profiles. Append `--source-only` to check source files without resolving the profile graphs. CI runs the full gate on Linux, macOS and Windows.

The D-CI-01-T02 negative graph test adds the workspace's `eframe` dependency to the CLI in a disposable copy and runs the same architecture command. It requires an `eframe` violation for each of the minimal, serial and default profiles. The workspace test suite includes this regression. [Mutation test](../../xtask/tests/dependency_mutation.rs)

Linux/macOS/Windows: fmt, lint, unit/integration, CLI `--no-default-features`, default `build+parallel`, serial `build`, each independent opt-in and prerequisites, read-only inspect/extract, default features + legacy (when implemented), optional backend features per target, GUI/MCP build/smoke, `--all-features` per package/target. Test disabled-command diagnostics and explicit feature requirements. `cargo tree -e features` and `cargo metadata` audit normal deps for GUI, network, signing, mount/notify, legacy, third-party core engines, unrelated FS; exception ledger for inseparable upstream internals. Test CLI output with no TTY and progress to stderr. No default Apple executables: run build on Linux/Windows and macOS environment without optional tool use; integration checks external programs not spawned. Separate dependency/license checks and security advisories; no blanket allowlist of unreviewed licenses. Fuzz via cargo-fuzz (nightly separate job), seeds from tiny valid profiles and hand-mutated cases, capped RSS/time, fail crashes/hangs and minimize regression inputs. Test Rust MSRV and pinned toolchain after dependency resolution. Publish signed releases only on protected workflows. Architecture DAG and schema checks in this documentation pack are not replacement for code tests.

## Specific acceptance priorities

- UDIF: independent checksum ledger for table vs physical vs master including zero/ignore, mixed raw/zlib, truncation, overflow, unknown run, comment/end, signed trailers, all-zero and multi-table. Read range near boundaries and across cache eviction; nonseekable output; malformed XML/base64, chunk count/decompression length limits. Exact decoded raw bytes on independent parsers + native verify.
- Build: 2+ apps, arbitrary nested supporting assets, hidden file vs dotfile, symlink, hard links, resource forks/attrs, Unicode conflicts, permission bits, input changed, source/output alias, insufficient capacity, strict loss report, no-clobber and rollback, serial/parallel exact unsigned output for same canonical profile, no full raw image, bounded working memory.
- Inspection/extract: no mount, lazy range access, clear unsupported profile, corrupt fixture diagnostics, complete verify vs quick, unknown metadata, Windows reserved names, escape/races, no silently dropped metadata, partial failure reporting.
- GUI: actual DMG source (not just project), root/nested views, fallback icons, keyboard accessibility, drag positions, no launch, comment-preserving config, undo/redo, dry-run, memory limits, no driver; headless preview dimensions and actual image decode.
- Mount/install: optional backends, verify identity, busy detach, preexisting identical app, multiple applications, canceled/slow copy and lost events, no forced eject; virtual install closes only after successful verified copy.
- Signing/MCP/remote: stage before signing, no source change, redacted credentials, rejected/pending/resumed notarization, exact artifact digest, backend-specific status, native checks explicit; path-scoped approvals, no arbitrary execution, source changes invalidate approval, range response/ETag/redirect/network caps, SSRF tests.

## Evidence status and release blockers

`planned`, `pass`, `fail`, `unsupported`, `unavailable`, `not_run` recorded per test/profile/host/toolchain; not-run cannot satisfy required gates. Store evidence artifact digest and logs; preserve versioned schema and capability matrices. All catalog entries in this pack are planned. A successful documentation validator means coverage and consistency, not implemented Rust, working DMGs, native approval, or measured speed. Hard release blockers: panic or unbounded resource use from supported/adversarial fixture, corruption, invalid signatures after packaging, silent metadata loss, incorrect checksums, default forbidden dependency, unsafe extraction or unmount, secret leakage, false success when feature absent. Unknown format behavior stays unsupported until qualified. See `docs/reference/test-catalog.json` for concrete cases and linked requirement IDs.

## Verification integration

The local [verification plan](VERIFICATION.md) and `verification/obligations.json` define mandatory profile-specific verification gates in addition to the original acceptance tests. Apply ecosystem standard EVS-1 (copy `shared/VERIFICATION.md` as `docs/architecture/ECOSYSTEM_VERIFICATION.md`). No required proof, model check, native test or critical fault gate can be substituted by coverage or a self-generated round trip. `cargo-nextest`, property tests, fuzz/corpus replay, coverage and deterministic architecture checks are required; formal methods activate at the milestones in the obligation registry.
