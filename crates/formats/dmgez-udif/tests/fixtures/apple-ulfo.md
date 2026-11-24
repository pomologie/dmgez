# Apple ULFO fixture

`apple-ulfo.dmg` is a single-table unencrypted image with a synthetic raw payload. It was generated on macOS 14.7.2 (23H311), arm64, with Apple DiskImages after wrapping the payload as UDRO. The fixture contains no filesystem or user data. It covers the framing, decoded size and container checksum parts of D-UDIF-05-T01 and D-UDIF-05-T02.

```sh
python3 - <<'PY'
from pathlib import Path
chunk_bytes = 1 << 20
sector_bytes = 512
Path('source.raw').write_bytes(
    b'ABCD' * (chunk_bytes // 4) + bytes(chunk_bytes) + b'ULFO' * (sector_bytes // 4)
)
PY
dmgez wrap-raw source.raw --format udro -o source.dmg
hdiutil convert source.dmg -format ULFO -o apple-ulfo.dmg
hdiutil verify apple-ulfo.dmg
```

| File | SHA-256 |
| --- | --- |
| `source.raw` | `5cacbd0c8bfb5d23e62db4df297ff7c075f54fe6c538eea507bb0bdb18085a1a` |
| `apple-ulfo.dmg` | `260e330343765e34fe96333828f33394021566185945668e975109fc12a96aa5` |

The block map contains three `0x8000_0007` runs with logical sector counts 2048, 2048 and 1. Their stored sizes are 769, 759 and 39 bytes. The large runs start with `bvx2` and the tail starts with `bvxn`. Each run ends with `bvx$`. Apple reported 2050 buffer sectors, equal to the largest decoded sector count plus its stored byte count rounded up to whole sectors. A separate larger native probe with high-entropy input emitted an expanding run and reported 4117 buffer sectors by the same rule.

The writer uses the `lzfse_rust` 0.2.1 ring encoder behind a fixed-size output slice, with raw fallback once the encoded stream no longer saves space. The 3 MiB per-worker encoder charge covers the pinned upstream fixed input and output rings, history table, and FSE workspaces separately from both payload buffers. The bounded ring decoder writes into a fixed extent and must report exact input consumption and decoded length. Both adapters call safe Rust APIs. [Upstream codec](https://docs.rs/lzfse_rust/0.2.1/lzfse_rust/)

Apple verification and UDTO export accepted an independently generated 4 MiB test image containing LZFSE, raw and explicit zero runs. The native export matched its original source byte-for-byte. The Rust decoder also exported an Apple-generated version of that input byte-for-byte and verified all declared CRC domains.
