# dmgez-mcp

`dmgez-mcp` is a separate local stdio server for MCP clients. It uses the shared dmgez TOML parser to validate project manifests on macOS and Linux.

```sh
cargo build --locked -p dmgez-mcp
target/debug/dmgez-mcp --project-root "$(pwd -P)"
```

Configure the client to launch the binary with `--project-root` and the physical path of the directory you authorize. The server holds that directory open for the session. `project_validate` takes `file_name`, a single `.toml` name immediately inside the directory. It rejects absolute paths, subdirectories, symlinks, multiply linked files, and nonregular files. Manifest paths such as source assets and output destinations are parsed as settings without being opened. The response contains a versioned status, a content count for a valid manifest, or a diagnostic code for an invalid one. The server returns no document text or paths from the document.

`capabilities` reports the tool and manifest schema versions and the request and file limits. The server accepts a 1 MiB manifest, 32 KiB stdio frame, 32 outstanding requests, and one validation at a time. The transport pauses reading while the output pipe has reached the outstanding request limit. The client can retry a `busy` validation response later. Both tools are read-only. The MCP capability list contains only the two implemented tools.

The stdio integration tests cover part of D-OPS-04-T01, including the MCP initialization handshake, typed schemas, denied mutations, path confinement, untrusted text, and size limits. D-OPS-04-T02 depends on a future approved build tool. The current package serves on macOS and Linux. Windows support needs a separately qualified handle-relative file adapter.

```sh
cargo test --locked -p dmgez-mcp
```
