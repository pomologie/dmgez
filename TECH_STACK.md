# dmgez technology and dependency policy

Rust edition 2024 with an explicitly pinned stable toolchain selected during bootstrap; record tested MSRV from resolved dependency graph (do not invent a version). Cargo resolver 3, locked application builds, `cargo fmt`, `clippy`, `cargo-deny`, `cargo-audit`, `cargo-nextest` (required), `cargo-fuzz` and `proptest`, Criterion or Divan benchmarks after measuring overhead. CI runs format/lint/test per supported target and feature combination. Publish packages only after checking registry availability. `no_std + alloc` is mandatory for standalone kernel-facing filesystem engines, not dmgez (including UDIF), its desktop app or hosted tooling. Prefer safe byte parsing, explicit checked endian operations; avoid `unsafe` in format code. Audit every FFI boundary.

| Area | Selected/candidate dependency | Runtime scope / qualification |
|---|---|---|
| UDIF | owned `dmgez-udif` | No third-party UDIF codec in primary engine |
| HFS+/HFSX | own independent hfsplus repository | builder and reader features independently |
| APFS | own independent apfs repository | opt-in later; no v1.0 blocker |
| Plist | `plist` | capped XML/binary parsing, deterministic writer |
| Compression | `flate2` with `default-features=false`, qualified `zlib-rs` backend | UDZO default; pin profile/versions; compare with native outputs |
| Checksums | `crc32fast` | standard CRC primitive; UDIF coverage/domain owned |
| Parallel | `rayon` | optional, default on; private worker pool and bounded queue |
| Finder | `ds_parser`, `alias_record` (verify crate/lib naming), `icns` | qualification against Finder fixtures; preserve unknown records; license review incl BlueOak |
| Config | `serde`, `toml`, `toml_edit` | typed strict deserialization; GUI comment-preserving edits |
| Manifest | `walkdir`, `globset` | no implicit gitignore; explicit dotfile policy; no symlink following |
| Host attrs | `xattr`, platform bindings | native Unix attributes; Windows and sidecars explicitly handled |
| Temporary staging | `tempfile` | platform-specific commit/no-clobber verified |
| CLI | `clap`, `indicatif` | optional terminal UI; machine output stable; progress stderr |
| Modern codecs | `lzfse_rust`, pure Rust LZMA candidate | separate gates; exact on-disk framing requires oracle qualification; XZ wrapper is not automatically ULMO |
| Legacy codecs | `bzip2`, native ADC decoder if needed | legacy only; choose Rust implementation if available and qualified |
| Partition maps | `gpt`, `mbrman`, `part-tables` APM or focused candidate | optional, raw partition windows; validate independently |
| Image processing | `png` 0.18.1, `tiff` 0.11.3, `icns`; `image` with only required codecs for GUI | Bounded Finder background capture and TIFF encoding under `finder-write`. TIFF enables LZW and Deflate for prebuilt assets. Optional `tiny-skia`/font renderer for preview. |
| Hashing | `blake3` | diff, cache, metadata verification; not UDIF checksum replacement |
| GUI | `egui`, `eframe`, `egui_extras`, `rfd`; native OS bindings for drag/drop | separate app, no embedded browser; own themed layout |
| Mount | `fuser`, `winfsp`, system tools/macOS APIs, `notify` | opt-in OS adapters; drivers may be external requirements |
| Signing | `apple-codesign` and Apple command wrappers | opt-in, qualified backends; transitive apple-dmg allowed here only if dependency requires it |
| Remote | `reqwest` or smaller TLS HTTP client with range support | optional; identity/validator and network budget enforcement |
| MCP | `rmcp` official Rust SDK, minimal async runtime required by transport | separate local stdio server |
| Encryption | vetted crypto libraries e.g. RustCrypto AES/HMAC/KDF, zeroize | stretch; implement only format envelope, never invent ciphers |

Dependencies listed are architectural selections or candidates, not a tested lockfile. Verify licenses, security advisories, platform compatibility, feature names and API coverage at a pinned release before adding. Focused external libraries may be replaced only if documented gaps/benchmarks justify owning independent functionality. If `.DS_Store` or alias dependencies bundle extra readers, measure code-size cost, disable supported features or contribute upstream; do not promise zero reader code until the dependency graph proves it. Avoid unrelated formats via `default-features = false`. `fstool`, `udif`, `apple-dmg` and third-party filesystem readers are development references/oracle adapters only, kept out of production builder graph. Optional signing may bring a transitive container library but must not substitute our writer/reader.

## Verification integration

The local `VERIFICATION.md` and `verification/obligations.json` define mandatory profile-specific verification gates in addition to the original acceptance tests. Apply ecosystem standard EVS-1 (copy `shared/VERIFICATION.md` as `docs/architecture/ECOSYSTEM_VERIFICATION.md`). No required proof, model check, native test or critical fault gate can be substituted by coverage or a self-generated round trip. `cargo-nextest`, property tests, fuzz/corpus replay, coverage and deterministic architecture checks are required; formal methods activate at the milestones in the obligation registry.
