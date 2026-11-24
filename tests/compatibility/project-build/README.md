# HFS+ build compatibility

Run this development exercise against adjacent source checkouts with an existing Cargo cache.

```sh
python3 run.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus
python3 run.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus --project /path/to/project.toml
python3 run.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus \
  --project /path/to/project.toml --define VERSION=2.0 \
  --set image.output=Example.dmg --dry-run
```

Use `--hfsplus-packages /path/to/unpacked` to select an unpacked set of versioned `hfsplus-builder`, `hfsplus-format`, and `hfsplus-core` 0.1.0 archives that include the chain-free hard-link builder API. The runner resolves paths before Cargo and confines path patches and source copies to a temporary Cargo project. Use `--no-default-features` for the serial profile without Finder store generation.

`--define NAME=VALUE` supplies a literal project substitution. `--set section.field=value` overrides one allowlisted scalar with normal config type and semantic validation. The last value for a repeated name or key wins. Relative TOML paths, including paths built from substitutions, use the config file's directory. The `--project` path and relative path values supplied directly with `--set` use the caller's working directory.

`--dry-run` reports the exact planned virtual size, requested encoding, enabled layout profile, publication and release intent, source counts and metadata-loss count. The physical UDIF size is unknown until encoding. The planner checks source identity, capacity and catalog collisions during dry-run. It leaves the output path and its directory untouched. Progress and up to twenty metadata-loss details go to stderr, with a count of further losses.

The default publication policy preserves an existing destination. Authorize replacement with `--overwrite`, `--set build.overwrite=true`, or `[build] overwrite = true` in the project. Choose a single CLI form for this setting. Use replacement only in a trusted local directory where publishers cooperate with the advisory lock. The builder checks source and destination identity, syncs the completed stage, and atomically replaces the destination. The stdout build summary follows successful publication.

The adapter uses dmgez preflight, source discovery and identity checks, Finder store generation, and staged publication. It binds generated payloads after HFS+ assigns catalog IDs, selects capacity with the same HFS+ planner used for placement, and streams the resulting image through UDIF. The manifest and sealed plan carry borrowed regular-source identities. Each bounded file read acquires an identity-checked handle and releases it before proceeding.

The qualified source profile covers ordinary regular file permissions and ownership, timestamp conversion at HFS+ precision, inline attributes, Finder information, a buffered resource fork, and exact symlink target bytes. The Unicode profile gives included regular-file hard links distinct catalog entries that share one chain-free inode, with both forks and attributes captured once. Source checks cover every alias before publication. Shared metadata losses name the representative and its other image paths. The strict policy rejects unavailable required directory and symlink attributes, subsecond date loss, unsupported BSD flags and attribute encodings, and hard-link groups outside the supported regular-file profile. Report policy records those losses, and an explicit hard-link preservation opt-out emits independent files with omission diagnostics. Explicit timestamp omission and an explicit generated epoch suit deterministic synthetic fixtures.

The smoke tests assert minimum free capacity, byte-exact regular-file, inline attribute and macOS resource-fork read-back, an insufficient shrink bound, and rejection of source changes before publication. Hard-link tests verify a shared inode and single attribute owner, distinct catalog IDs, independent equal-content files, byte-identical output after reordering declarations, and rejection when the nonrepresentative source path is replaced. Native smoke evidence additionally runs Apple's UDIF checksum verifier and UDTO export on a disposable generated image, checks exact file and symlink bytes with 7-Zip, and reads both Finder positions with the Python ds_store oracle.

A separate read-only mount on macOS 14.7.2 verified that both hard-link aliases had the same inode and link count of two. It also checked the independent file's inode, executable mode, and both aliases' exact data, resource fork, and inline attribute.
