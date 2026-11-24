# UDIF v4 bootstrap evidence

These observations apply to unsigned single-segment UDIF v4 files with XML resources. The fixtures were generated on macOS 14.7.2 (23H311, arm64) using synthetic sector patterns and a synthetic HFS+ volume. The [fixture manifests](../../tests/fixtures/manifests) contain native tool arguments, hashes, geometry, exact run records and independently computed checksum values. `generate_udif.py` uses Python's `plistlib`, `struct` and `zlib` independently of the Rust implementation.

| Record | Field or domain | Observed values |
|---|---|---|
| `koly` | Version at 4, header length at 8, flags at 12 | 4, 512, 1 |
| `koly` | Running data offset at 16, data offset at 24, data length at 32 | Single-segment fixtures use zero running offset. Data-fork byte offsets are file-relative. |
| `koly` | Resource fork at 40 and 48, segment number/count at 56 and 60, segment ID at 64 | Absent resource fork, segment 1 of 1, 16 opaque identity bytes |
| `koly` | CRC descriptor at 80, XML offset/length at 216 and 224 | Type 2, width 32, value at 88. CRC covers exactly the stored data fork. XML is outside the data fork. |
| `koly` | Master descriptor at 352, variant at 488, disk sector count at 492 | Value at 360 is CRC32 of the ordered table CRCs encoded big-endian. Variant 2 for the single-map raw fixtures and variant 1 for the APM fixture. |
| `mish` | Version at 4, logical first sector/count at 8 and 16, data base at 24 | Version 1. Run start is table-relative; physical offsets are normalized against the table base and data fork. Native fixtures use zero table base. |
| `mish` | Buffer hint at 32, descriptor at 36, checksum at 64 | Native zlib chunks of 2048 sectors declare 2056 buffer sectors. Raw-only and all-ignore tables declare zero. Checksum value at 72 covers decoded table data in run order. |
| `mish` | Record count at 200, rows at 204 | Forty bytes per row, big-endian `u32`, `u32`, followed by four `u64` fields for start, count, stored offset and stored length. |

| Run | Logical behavior | Table CRC |
|---|---|---|
| `0x00000000` | Explicit zero bytes, no stored payload | Include declared zero bytes |
| `0x00000001` | Read the exact stored bytes | Include stored logical bytes |
| `0x00000002` | Ignore region, exposed as zero in raw export | Exclude the region entirely |
| `0x80000005` | Decode one exact zlib stream | Include decoded logical bytes |
| `0x80000008` | Decode one qualified single-block LZMA2/XZ stream | Include decoded logical bytes |
| `0x7ffffffe` and `0xffffffff` | Comment and terminator, no logical payload | Exclude |

Formats are corroborated by the [libmodi format register](https://github.com/libyal/libmodi/blob/main/documentation/Mac%20OS%20disk%20image%20types.asciidoc). Apple `hdiutil verify` accepted the explicit zero, ignore and comment probes. It rejected the negative zero probe whose checksum excluded the zero bytes. Apple-generated APM UDZO provided independently observed ignore runs in stored and free partitions. Export preserves the declared logical disk size, including the trailing ignore region.

## Verification scope

`tests/compatibility/verify_udif.py` compares Apple-generated and dmgez-generated images for tiny, mixed and all-zero input in UDRO and UDZO. Exported bytes matched through both Apple and 7-Zip (17.05), and Apple verified the emitted checksums. With `--attach-hfs`, macOS attached and mounted a dmgez UDZO containing an Apple-generated clean partitionless HFS+ volume. `fsck_hfs -fn` reported the attached volume clean. This evidence covers the UDIF layer and Apple-generated payload. The native HFS+ constructor and the broader host and oracle matrix remain open gates.

The checked-in corpus and its malformed probes cover parts of D-UDIF-01 through D-UDIF-04 and D-V01 through D-V04. Signed resources, encryption, segmentation, binary plists, other compressors and filesystem-level checks remain outside the implemented profile. The initial raw wrap CLI accepts regular files. The writer API accepts a finite stream and a caller-owned sink.

The optional `lzma` profile reads and writes [Apple-qualified ULMO streams](../../crates/formats/dmgez-udif/tests/fixtures/apple-ulmo.md). The raw wrap CLI, TOML builder preflight, and inspector share this profile. Inspection identifies the run encoding even when payload decoding is disabled.

The macOS bootstrap run passed 16 nextest cases and the Kani 0.68.0 checked-range harness over arbitrary `u64` inputs. The nightly integrated parser fuzz smoke completed 803,954 runs in 21 seconds using the six native fixtures as seeds. `cargo-llvm-cov` reported 91.34% line coverage for the hosted all-feature workspace. Several critical format modules remain below the proposed per-module threshold. Linux x86-64 and Windows arm64 passed target `cargo check`. Native Linux and Windows execution, the implementation-linked Lean pilot, and complete release evidence remain open.
