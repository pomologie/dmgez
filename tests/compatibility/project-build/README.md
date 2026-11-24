# HFS+ build compatibility

Run this development exercise against adjacent source checkouts with an existing Cargo cache.

```sh
python3 run.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus
python3 run.py --dmgez /path/to/dmgez --hfsplus /path/to/hfsplus --project /path/to/project.toml
```

Use `--hfsplus-packages /path/to/unpacked` to select an unpacked set of versioned `hfsplus-builder`, `hfsplus-format`, and `hfsplus-core` 0.1.0 archives. The runner resolves paths before Cargo and confines path patches and source copies to a temporary Cargo project. Use `--no-default-features` for the serial profile without Finder store generation. The project build uses its configured output and fails if that path already exists.

The adapter uses dmgez preflight, source discovery and identity checks, Finder store generation, and staged publication. It binds generated payloads after HFS+ assigns catalog IDs, selects capacity with the same HFS+ planner used for placement, and streams the resulting image through UDIF. The manifest and sealed plan carry borrowed regular-source identities. Each bounded file read acquires an identity-checked handle and releases it before proceeding.

The qualified source profile covers ordinary regular file permissions and ownership, timestamp conversion at HFS+ precision, inline attributes, Finder information, a buffered resource fork, and exact symlink target bytes. Source directory and link attributes remain unavailable to this capture path. The strict policy rejects those unavailable required attributes, subsecond date loss, unsupported BSD flags, unsupported attribute encodings and included hard-link groups. Report policy records those losses. Explicit timestamp omission and an explicit generated epoch suit deterministic synthetic fixtures.

The smoke tests assert minimum free capacity, byte-exact read-back of a regular file and inline attribute, macOS resource-fork bytes, deterministic output from one prepared manifest, source mutation refusal before publication, and an insufficient shrink bound. Native smoke evidence additionally runs Apple's UDIF checksum verifier and UDTO export on a disposable generated image, checks exact file and symlink bytes with 7-Zip, and reads both Finder positions with the Python ds_store oracle.
