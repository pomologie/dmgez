# dmgez-mcp

`dmgez-mcp` is a separate local stdio server for MCP clients on macOS, Linux, and Windows. It lists project candidates, reads and validates manifest summaries, and previews project edits with the shared dmgez project editor.

```sh
cargo build --locked -p dmgez-mcp
target/debug/dmgez-mcp --project-root "$(pwd -P)"
```

Configure the client to launch the binary with `--project-root` and the physical path of the directory you authorize. The server holds that directory open for the session. `project_read`, `project_validate`, and `project_update` take `file_name`, a single `.toml` name immediately inside the directory. These tools reject absolute paths, subdirectories, symlinks, multiply linked files, and nonregular files. Source and output paths are parsed as settings without being opened. Responses include schema versions and diagnostic codes.

Windows requires an ordinary path on a physical local NTFS drive, such as `C:\Work\Projects`. The root walk rejects reparse points, including junctions, and file requests require the exact stored spelling. Alternate streams, UNC paths, and substituted drives are outside this profile. The same rules apply to `--image-root`.

`project_list` lists candidate manifest names in UTF-8 byte order from direct directory entries and their file types. Supply `page_size` from 1 through 16 or omit it for 16. Pass the returned `next_after` value as `after` for the next page. Each page observes the directory at request time. File additions or removals between calls can affect later pages. The scanner stops with `directory_scan_limit` if the directory exceeds its 4096-entry scan budget.

`project_read` returns the SHA-256 of the exact admitted file bytes, byte count, content count, image format, filesystem, and reproducibility setting. `project_validate` returns the content count for a valid manifest.

### Preview project edits

Call `project_update` with `file_name`, `expected_revision_sha256` from `project_read`, and an ordered `edits` array of one to 128 objects. The revision is mandatory and uses 64 lowercase hexadecimal characters. The server compares it with the exact bytes of the admitted manifest. A mismatch returns `stale_revision` with the digest it observed.

| `operation` | Fields |
| --- | --- |
| `add_copy` | `source` and optional `destination` |
| `add_directory` | `destination` |
| `add_symlink` | `destination` and `target` |
| `modify` | `target` and optional `destination`, `position`, `hidden`, and `hide_extension` |
| `remove` | `target` |
| `set_image_name` | `name` |

Image destinations and selectors use absolute slash-delimited image paths. `add_symlink` stores its `target` string in the link. `modify` and `remove` select declarations by their destinations at the start of the batch. For `position`, `hidden`, and `hide_extension`, omit a field to keep its setting or pass `null` to clear it. Supply two finite coordinates for `position`, and JSON booleans for the display flags.

The tool validates the whole batch in memory under the same 1 MiB limit for the original and proposed manifest. A `planned` result includes the source and proposed SHA-256 digests, manifest byte counts, and content entry counts. `difference` gives a UTF-8 boundary as the starting byte offset and the length of the single span covering all changes in each revision. It is `null` when the bytes match. The report contains no manifest text. The manifest read is the tool's only file access. [Project update tests](tests/stdio/update.rs)

### Inspect images

Supply `--image-root /physical/path/to/images` to authorize a separate directory for the read-only `image_inspect` and `image_verify` tools. Both take `file_name`, a single `.dmg` name in that directory. The image root is held open for the session and uses the same direct regular-file admission as the project root. The server lists the image tools only when the image root is configured.

`image_inspect` summarizes the admitted UDIF container with byte counts and block map and run counts. Supported signed images include an `embedded_signature` object with numeric `offset_bytes` and `length_bytes` fields. `image_verify` checks the trailer, block maps, and any supported signature frame. The `payload`, `filesystem`, and `security` coverage fields remain `not_checked`. Both tools allow up to 1 MiB for XML and signature bytes combined, plus the 512-byte trailer, across at most three reads. The reader checks the declared total before allocating frame or XML buffers. [Signed image tests](tests/stdio/images.rs)

`capabilities` reports schema versions, resource limits, and whether image inspection is available. The server accepts a 1 MiB manifest, a 32 KiB incoming stdio frame, 32 outstanding requests, and one local file operation at a time. The transport pauses reading while the output pipe has reached the outstanding request limit. The client can retry a `busy` response later. All tools are read-only.

The stdio integration tests cover part of D-OPS-04-T01, including the MCP initialization handshake, typed schemas, denied mutations, path confinement, untrusted text, bounded listing, exact revision checks and dry-run diffs, and confined UDIF metadata reads. The Windows-only cases cover exact names, alternate streams, junctions, and refused root path types. The Windows build passed the x64 cross-target check. Native NTFS execution is covered by the Windows CI test job and awaits a recorded run. D-OPS-04-T02 depends on a future approved build tool.

```sh
cargo test --locked -p dmgez-mcp
```
