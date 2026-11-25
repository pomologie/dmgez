# dmgez architecture

## 1. Product boundary

A native Rust DMG toolkit with a lean default build, rich optional read/manipulation services, a desktop studio, and a local MCP interface. Standard build inputs are arbitrary files, directories and bundles with Finder layout and metadata; standard output is an ordinary macOS-mountable HFS+ UDIF image. It must work on macOS, Linux and Windows without Apple executables. CLI, GUI and MCP share exactly one project model and service layer. dmgez owns UDIF semantics, DMG-specific orchestration, and user experiences. Independently reusable HFS+ and APFS engines are separate projects. Do not use fstool, udif or apple-dmg as production image or filesystem backends. Do not create competing generic libraries for CRC, plists, compression, partition tables, or icon metadata unless measured shortcomings justify it.

### Users and profiles

- App publisher and Dockhand: reproducible HFS+ builds, Finder style, no privileged mounts, CI-friendly stdout/status.
- Disk image developer: detailed structure, verified reads, exact offsets, filesystem internals, differential tests and diagnostics.
- Desktop user: browse actual images, visually design/edit, extract and optionally install without a kernel mount.
- Advanced administrator: optional OS-backed read-only mount, explicitly managed auto-ejection, signed artifacts, remote inspection.
- Charles: reusable storage-format components independent of macOS. Charles is never a DMG milestone dependency.

Out of scope: filesystem-aware FAT32/exFAT/NTFS creation, physical disk partitioning or repair, deleted-file carving/recovery, macOS runtime emulation, automatic execution on image attach, bypass of code signing or DRM, arbitrary Python configuration execution. Raw-sector conversion may preserve unknown filesystems opaquely without reading or writing their logical contents. Encryption, writable mounts, APFS integration and old container families are nonblocking stretch features.

## 2. Target workspace

```text
dmgez/
  apps/
    dmgez/                       # dmgez-gui binary
    dmgez-cli/                   # dmgez binary (default workspace member)
    dmgez-mcp/                   # dmgez-mcp binary, optional stdio service
  crates/
    foundation/
      dmgez-core/                # contracts, typed paths, errors, budgets, events
      dmgez-config/              # typed versioned TOML and validation
      dmgez-vfs/                 # composition of read-only image + filesystem views
    formats/
      dmgez-macho/               # bounded inspection of executable slices
      dmgez-udif/                # independently owned DMG-specific format
      dmgez-legacy/              # stretch; NDIF/DC42 etc., not a generic codec crate
      dmgez-sparse/              # stretch; sparse/sparsebundle/ASIF separate modules
    presentation/
      dmgez-finder/              # DMG layout + generated records/assets orchestration
      dmgez-preview/             # shared preview geometry + optional renderer
    services/
      dmgez-builder/             # manifest and image construction orchestration
      dmgez-hfsplus/             # compose the independent HFS+ builder and DMG publication
      dmgez-hfsplus-read/        # adapter for HFS+ browsing and structural verification
      dmgez-apfs-read/           # APFS browsing, reachable verification and quarantine admission
      dmgez-inspector/           # structured read-only reports / verification
      dmgez-operations/          # extraction, conversion, copy-on-write edits
      dmgez-diff/                # semantic and physical comparison, once substantial
      dmgez-lint/                # policy-based diagnostics and release checks
      dmgez-cache/               # DMG-specific block reuse, optional
    platform/
      dmgez-mount/               # OS backends and managed sessions
      dmgez-install/             # explicit bundle copy/install and safe monitoring
    security/
      dmgez-signing/             # integration of Apple tools and apple-codesign
      dmgez-encryption/          # format-specific envelope, stretch
  tests/fixtures/               # synthetic fixtures and provenance manifests
  tests/differential/           # standalone test package or per-crate test targets
  tests/compatibility/          # macOS oracle scripts and results
  benches/  fuzz/  docs/  examples/  assets/  xtask/  .github/workflows/
  Cargo.toml  Cargo.lock  rust-toolchain.toml  README.md  AGENTS.md
```

Directory names and package names are fully prefixed. Only scaffold core/config/udif/builder and CLI when actually required. Other names are target responsibilities, not mandates for immediate crate creation; a small module can become a crate when its dependency, publishing, testing or security boundary warrants it. HFS+ and APFS source live in separate repositories. APM/GPT/MBR, aliases, DS Store, ICNS and codecs are focused dependencies rather than owned dmgez subprojects. The GUI package name must not collide with the CLI's `dmgez` executable (use `dmgez-gui` package/binary; display name dmgez).

## 3. Directed dependency graph

Read arrows as `consumer -> dependency`.

| Consumer | Permitted first-party dependencies |
|---|---|
| `dmgez-core` | none; no host UI, networking, formats or services |
| `dmgez-config` | core |
| `dmgez-udif` | core (or its own minimal portable format types); no filesystem |
| `dmgez-macho` | none |
| `dmgez-finder` | core, config; external Finder libraries; no concrete builder |
| `dmgez-builder` | core, config, udif writer, finder writer; Mach-O inspection for optional volume icons |
| `dmgez-hfsplus` | dmgez-builder, core, config, udif writer, independent versioned HFS+ libraries, Finder store editing for existing images |
| `dmgez-hfsplus-read` | independent versioned HFS+ reader and format libraries, vfs for browsing |
| `dmgez-apfs-read` | independent versioned APFS container, reader and format libraries, vfs for browsing and verification |
| `dmgez-vfs` | core, udif reader, independent filesystem readers, partition adapters; no write service |
| `dmgez-inspector` | core, vfs, udif reader, finder reader; metadata-only mode can avoid traversal |
| `dmgez-operations` | core, vfs, writer/reader and builder when a requested operation needs them |
| `dmgez-preview` | core, finder, config; VFS adapter for actual images optional; no GUI framework required in headless mode |
| `dmgez-diff` | vfs for semantic comparison, optional Finder store reader and BLAKE3 hashing |
| `dmgez-lint` | preview geometry for layout, optional builder manifest discovery for installer inputs, signing checks only via optional integration |
| `dmgez-cache` | core and codec-profile contracts; builder can consume it; never import builder |
| `dmgez-install` | core, operations extraction, platform adapters; no GUI |
| `dmgez-mount` | core, vfs, optional install-monitoring policy; no GUI |
| `dmgez-signing` | optional signing integrations, Mach-O inspection, UDIF trailer admission and signature append validation, core publication for notarization records; no builder import |
| `dmgez-encryption` | core, generic crypto primitives; exposed as a bounded storage adapter |
| apps | compose selected services, not one another |

For release workflows the app (or a later shared DMG release service) composes `builder -> signing -> notarization -> verification`; the builder does not import signing or network credentials. A lower-level module exposes a typed event/contract rather than invoking a higher service. No dependency cycle through `core`, and no backend detection by trying arbitrary executable paths. Runtime capability reports distinguish compiled vs available. Contract revisions C1-C8 are defined in the ecosystem contracts. A library consumer can use UDIF directly to wrap a known-length raw block source without linking config, HFS+ or Finder.

## 4. Build profiles and feature forwarding

Canonical CLI capability graph is in `docs/reference/feature-graph.json`; the graph describes the target release profile. Implement only gates whose functionality actually exists. `[workspace] resolver = "3"`, Rust edition 2024; `default-members = ["apps/dmgez-cli"]`. Each optional service dependency uses `optional = true` and `default-features = false` where appropriate; each CLI feature forwards to the owning package feature. The common default CLI `build,parallel` enables `udif-write,hfsplus-build,finder-write,zlib` and required shared plumbing. Once qualified, ordinary Linux and Windows CLI builds also include `signing-rust`, and `auto` selects it on those hosts. macOS keeps separate opt-in signing backends. Use `cargo build -p dmgez-cli --release --no-default-features --features build` for serial. Reader and write APIs should gate separately, including separate feature gates on filesystem dependencies. Never expose an option as successfully usable when compiled out. Compile a minimal help/capabilities CLI with no defaults. A library-only build does not include CLI, rayon, GUI or MCP unless selected.

While the exact HFS+ dependencies are unpublished, keep their service outside the normal workspace lock. Use a temporary Cargo workspace with explicit patches to the sibling sources or unpacked packages to compile the same standalone CLI and build command. The release workspace will resolve the versioned dependencies directly.

- `parallel` enables bounded worker scheduling; `build` alone works and selects serial. Enabling `parallel` alone can expose compression scheduling primitives without implicitly enabling a high-level builder. TOML `performance.parallel = "auto"` chooses compiled support and actual resources; boolean true explicitly requires compiled parallel support, false always serial. `threads=0` resolves to a bounded host-aware count. Explicit threads >1 in serial-only mode error; no silent fallback for explicit requests.
- `inspect` enables reader + filesystem + partition detection + Finder reader, `verify` includes reader, filesystem structural verification when supported. `extract`, `diff`, `edit`, `preview`, `convert` depend on only needed readers or writers. `verify_output=true` is explicit and requires verification support; default writer performs internal checks, not full independent read-back. `verify` without supported filesystem reports partial container verification, not whole-image success.
- Separate optional codecs (`lzfse`,`lzma`); `legacy` is an off-by-default umbrella for obsolete format families and codecs. No legacy feature advertising until implemented. `license` is optional legacy resource feature, separate from container-family gate. `apfs-read` and `apfs-build` separate; APFS not default or v1.0 gate. `mount-system` only macOS, `mount-fuse` appropriate Unix targets, `mount-winfsp` only Windows; a feature may be present with a backend unavailable at runtime. Compile conditional dependencies with target `cfg`; full-feature build per platform must not link another platform's drivers. `mount-watch` requires at least one available backend at runtime and explicit paths. `signing-rust`, `signing-native`, `notarization-rust`, `notarization-native` are independent. Native signing is macOS-only. Timestamp and notarization requests require explicit release actions.
- GUI and MCP are separate binaries, each with own default service selections and independent optional features. `dmgez open` can invoke an explicitly configured/installed GUI using safe argument passing, or report unavailable; no linkage from default CLI to GUI. No GUI or MCP feature in core library.
- Build profiles: default builder; serial minimal builder; read-only inspector/extractor; release tool; desktop app; full supported platform CLI. `cargo tree -e features -p dmgez-cli` and `cargo metadata --filter-platform` enforce forbidden dependency categories, with an explicit reviewed exception ledger for inseparable internals of small upstream libraries. `--all-features` for an individual package, not root workspace, is the full supported build for a platform. Separate target jobs for Windows/macOS/Linux and `no_std` core tests in filesystem repos.

## 5. Service API and state ownership

Sketch, not stable code:

```rust
pub struct BuildRequest { /* validated Project, output intent, policy, budgets */ }
pub fn plan(req: BuildRequest, sources: &dyn SourceProvider) -> Result<BuildPlan>;
pub fn execute(plan: BuildPlan, target: OutputTarget, ctx: &OperationContext)
    -> Result<BuildReport>;
pub fn open_image(source: ImageSource, opts: ReadOptions) -> Result<ImageSession>;
// ImageSession: container report, partition views, volume views, capabilities.
// No mount, network or execution by default.
```

Use owned immutable plans, fallible builders and stream handles. `Project` is a logical configuration document, `ResolvedManifest` is a snapshot of intended entries and captured metadata, `ImagePlan` is final sector geometry; don't mutate one through another after validation. GUI undo/redo, CLI and MCP submit the same typed commands and validation policies. Edit an existing image via an overlay and a new output; an image session is immutable. `BuildReport`, `InspectionReport`, `VerificationReport`, `DiffReport` and `ReleaseReport` have `schema_version`, operation/image identity, capabilities, diagnostics and stage outcomes. Reports separate facts from inference. JSON and machine output preserve units and raw identifier representations; large integers represented as decimal strings where JavaScript precision would otherwise be lost. Errors use common contract C6.

## 6. Critical-path build and read flow

```
TOML/CLI/GUI -> validate capabilities -> resolve source tree and metadata
 -> canonical filesystem names + stable ids -> Finder generated assets/aliases
 -> independent HFS+ sealed plan -> logical sectors
 -> bounded UDIF encode + ordered output -> stage -> optional verification -> commit
```

No full raw image for normal HFS+ profile; temporary metadata spool or optional source snapshot is permitted and measured. Never write a nominally deterministic image when its input changes mid-build. `dmgez-udif` works over a finite raw byte source, and its reader provides a virtual byte source over decoded runs. Reader order: classify envelope/container; parse header/trailer and resource index; virtual disk bounds; partition detection (or unpartitioned volume); filesystem detection; open selected volume; retrieve catalog and Finder metadata on demand. Unsupported fs/codec/image profile remains inspectable at outer layers without pretending the contents were read. Multiple partitions, overlaps, hybrid layouts and APFS multiple volumes require explicit volume IDs; never pick the first silently if ambiguous. Mounting is an optional consumer of the same reader, not a prerequisite.

## 7. Quality and support boundaries

See [testing](TESTING.md) and the [roadmap](ROADMAP.md). Every capability has an owning specification and requirement/test IDs. First releases qualify a finite profile, not all DMG variations. Require native Apple validation on licensed macOS infrastructure, plus independent readers. Define deterministic unsigned artifacts only for exact config/source metadata, toolchain and codec profile; signing timestamps, tickets and host metadata can differ. No performance or Apple compatibility claims until measured. Security: untrusted bytes, names, plists, icons, remote images, external tools and MCP requests are boundary inputs. Default reader is read-only and never launches executables. No visual copying of Apple's proprietary artwork. IP policy and provenance follow C8; professional review before commercial advanced APFS or sensitive interoperability work.

## Verification integration

The local [verification plan](VERIFICATION.md) and `verification/obligations.json` define mandatory profile-specific verification gates in addition to the original acceptance tests. Apply ecosystem standard EVS-1 (copy `shared/VERIFICATION.md` as `docs/architecture/ECOSYSTEM_VERIFICATION.md`). No required proof, model check, native test or critical fault gate can be substituted by coverage or a self-generated round trip. `cargo-nextest`, property tests, fuzz/corpus replay, coverage and deterministic architecture checks are required; formal methods activate at the milestones in the obligation registry.

## Kernel portability boundary
dmgez is a std-hosted toolkit. HFS+/APFS supply independently testable no_std + alloc kernel-facing engines and may expose std bulk builders and adapters. APFS is intended for Charles kernel execution; dmgez need not wait for the kernel or its userspace std port. Keep filesystem semantics consistent across hosted and kernel profiles and coordinate sibling changes under EES-1.
