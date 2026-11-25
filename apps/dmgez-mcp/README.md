# dmgez-mcp

`dmgez-mcp` is a separate local stdio server for MCP clients on macOS and Linux. It lists project candidates, reads validated manifest summaries, and validates manifests with the shared dmgez TOML parser.

```sh
cargo build --locked -p dmgez-mcp
target/debug/dmgez-mcp --project-root "$(pwd -P)"
```

Configure the client to launch the binary with `--project-root` and the physical path of the directory you authorize. The server holds that directory open for the session. `project_read` and `project_validate` take `file_name`, a single `.toml` name immediately inside the directory. Both reject absolute paths, subdirectories, symlinks, multiply linked files, and nonregular files. Source and output paths are parsed as settings without being opened. Responses include schema versions and diagnostic codes.

`project_list` lists candidate manifest names in UTF-8 byte order from direct directory entries and their file types. Supply `page_size` from 1 through 16 or omit it for 16. Pass the returned `next_after` value as `after` for the next page. Each page observes the directory at request time. File additions or removals between calls can affect later pages. The scanner stops with `directory_scan_limit` if the directory exceeds its 4096-entry scan budget.

`project_read` returns the SHA-256 of the exact admitted file bytes, byte count, content count, image format, filesystem, and reproducibility setting. `project_validate` returns the content count for a valid manifest.

Supply `--image-root /physical/path/to/images` to authorize a separate directory for the read-only `image_inspect` and `image_verify` tools. Both take `file_name`, a single `.dmg` name in that directory. The image root is held open for the session and uses the same direct regular-file admission as the project root. The server lists the image tools only when the image root is configured.

`image_inspect` summarizes the admitted unsigned UDIF container with byte counts and block map and run counts. `image_verify` checks the trailer and block map structure in quick mode. Its coverage fields report payload, filesystem, and signature checks as `not_checked`. Both operations read at most two metadata ranges totaling 1 MiB plus the 512-byte trailer. Responses report numeric metadata and typed outcome codes.

`capabilities` reports schema versions, resource limits, and whether image inspection is available. The server accepts a 1 MiB manifest, a 32 KiB incoming stdio frame, 32 outstanding requests, and one local file operation at a time. The transport pauses reading while the output pipe has reached the outstanding request limit. The client can retry a `busy` response later. All tools are read-only.

The stdio integration tests cover part of D-OPS-04-T01, including the MCP initialization handshake, typed schemas, denied mutations, path confinement, untrusted text, bounded listing, manifest revision changes, and confined UDIF metadata reads. D-OPS-04-T02 depends on a future approved build tool. Windows support needs a separately qualified handle-relative file adapter.

```sh
cargo test --locked -p dmgez-mcp
```
