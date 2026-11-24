# Apple ULMO fixture

`apple-ulmo.dmg` is a single-table image built on macOS 14.7.2 (23H311), arm64, from the same synthetic payload as the [Apple ULFO fixture](apple-ulfo.md). Generate its `source.raw` and `source.dmg` using that recipe, then run `hdiutil convert source.dmg -format ULMO -o apple-ulmo.dmg` and `hdiutil verify apple-ulmo.dmg`. The image contains only synthetic sector bytes.

| File | SHA-256 |
| --- | --- |
| `source.raw` | `5cacbd0c8bfb5d23e62db4df297ff7c075f54fe6c538eea507bb0bdb18085a1a` |
| `apple-ulmo.dmg` | `7707bcc628c0214394b78b59eed276b72db271ca7f15c8c11ad749b43f4fb6e3` |

The block map has three `0x8000_0008` runs. Their decoded sector counts are 2048, 2048 and 1, and their encoded sizes are 284, 280 and 72 bytes. Each run contains an XZ stream with no payload checksum and one LZMA2 block. The whole-disk CRCs remain in UDIF. Apple reported 2049 buffer sectors, the largest decoded sector count plus that run's stored bytes rounded up to whole sectors.

An independent `xz --robot -lvv` check on a larger Apple-generated image showed 8 MiB dictionaries in three compressible runs and a 4 KiB dictionary in a run with high-entropy bytes. The latter expanded to 1,048,680 encoded bytes. The maximum combined decoded and encoded size was 4097 sectors, which matched Apple's table field.

The decoder admits the single-block LZMA2 profile, caps the dictionary at 8 MiB, and passes a 9 MiB decoder limit to the upstream XZ implementation. It requires exact input consumption and decoded length. The writer uses preset 1 with a 1 MiB dictionary and a fixed output slice. Its 10 MiB per-worker scratch charge covers the pinned encoder's estimated 8361 KiB workspace, separately from both payload buffers. [Pinned upstream codec](https://docs.rs/lzma-rust2/0.25.0/lzma_rust2/)

Rust verified and decoded the larger Apple image byte-for-byte, including the expanding run. Apple verified a Rust image from the same input with two LZMA2 runs, a zero run and a raw run. Apple UDTO export matched the original bytes. An independently constructed HFS+ volume also passed Apple verification and byte-exact UDTO export after being wrapped as ULMO by the Rust writer.
