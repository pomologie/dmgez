#!/usr/bin/env python3
"""Generate and independently describe synthetic UDIF fixtures with macOS hdiutil."""
import argparse
import hashlib
import json
import pathlib
import plistlib
import random
import struct
import subprocess
import zlib

# Observed UDIF v4 wire layout. Keep these definitions independent of dmgez.
SECTOR_BYTES = 512
TRAILER_BYTES = 512
TRAILER_SIGNATURE = b"koly"
TRAILER_DATA_OFFSET = 24
TRAILER_DATA_LENGTH = 32
TRAILER_DATA_CRC = 88
TRAILER_XML_OFFSET = 216
TRAILER_XML_LENGTH = 224
TRAILER_MASTER_CRC = 360
TRAILER_VARIANT = 488
TRAILER_SECTORS = 492

BLOCK_MAP_BYTES = 204
BLOCK_MAP_FIRST_SECTOR = 8
BLOCK_MAP_SECTORS = 16
BLOCK_MAP_DATA_BASE = 24
BLOCK_MAP_BUFFER_SECTORS = 32
BLOCK_MAP_CRC = 72
BLOCK_MAP_RUN_COUNT = 200

RESOURCE_FORK_KEY = "resource-fork"
BLOCK_MAPS_KEY = "blkx"
RESOURCE_DATA_KEY = "Data"
RESOURCE_ID_KEY = "ID"

RUN_ZERO = 0
RUN_RAW = 1
RUN_IGNORE = 2
RUN_ZLIB = 0x80000005
RUN_COMMENT = 0x7FFFFFFE
RUN_END = 0xFFFFFFFF
RUN_NO_COMMENT = 0
BE_U32 = struct.Struct(">I")
BE_U64 = struct.Struct(">Q")
RUN_RECORD = struct.Struct(">IIQQQQ")

HDIUTIL = "/usr/bin/hdiutil"
HDIUTIL_VERIFY = (HDIUTIL, "verify", "-quiet")
RAW_DISK_IMAGE_CLASS = "diskimage-class=CRawDiskImage"
UDRO = "UDRO"
UDZO = "UDZO"
VOLUME_NAME = "Pomologie Fixture"
TINY_CASE = "tiny"
TINY_DATA_START = 1
TINY_DATA_SECTORS = 2
BLOCK_BYTES = 1 << 20
MIXED_SEED = 1832


def capture(*args):
    return subprocess.check_output(args, text=True).strip()


def u32(data, offset):
    return BE_U32.unpack_from(data, offset)[0]


def u64(data, offset):
    return BE_U64.unpack_from(data, offset)[0]


def characterize(image, raw, generate, profile):
    payload = image.read_bytes()
    trailer = payload[-TRAILER_BYTES:]
    assert trailer.startswith(TRAILER_SIGNATURE)
    data_start = u64(trailer, TRAILER_DATA_OFFSET)
    data_length = u64(trailer, TRAILER_DATA_LENGTH)
    xml_start = u64(trailer, TRAILER_XML_OFFSET)
    xml_length = u64(trailer, TRAILER_XML_LENGTH)
    plist = plistlib.loads(payload[xml_start:xml_start + xml_length])
    tables = plist[RESOURCE_FORK_KEY][BLOCK_MAPS_KEY]
    assert u32(trailer, TRAILER_DATA_CRC) == zlib.crc32(payload[data_start:data_start + data_length])
    master = bytearray()
    expected_disk = bytearray(raw)
    evidence = []
    for entry in tables:
        table = entry[RESOURCE_DATA_KEY]
        first_sector = u64(table, BLOCK_MAP_FIRST_SECTOR)
        sectors = u64(table, BLOCK_MAP_SECTORS)
        base = u64(table, BLOCK_MAP_DATA_BASE)
        runs = [RUN_RECORD.unpack_from(table, BLOCK_MAP_BYTES + RUN_RECORD.size * index)
                for index in range(u32(table, BLOCK_MAP_RUN_COUNT))]
        crc = 0
        for kind, _, start, count, offset, length in runs:
            begin = (first_sector + start) * SECTOR_BYTES
            end = begin + count * SECTOR_BYTES
            source = payload[data_start + base + offset:data_start + base + offset + length]
            if kind in (RUN_ZERO, RUN_IGNORE):
                expected_disk[begin:end] = bytes(end - begin)
                if kind == RUN_ZERO:
                    crc = zlib.crc32(expected_disk[begin:end], crc)
            elif kind in (RUN_RAW, RUN_ZLIB):
                decoded = source if kind == RUN_RAW else zlib.decompress(source)
                assert len(decoded) == end - begin
                assert raw[begin:end] == decoded
                crc = zlib.crc32(decoded, crc)
            else:
                assert kind in (RUN_COMMENT, RUN_END)
        assert crc == u32(table, BLOCK_MAP_CRC)
        master += BE_U32.pack(crc)
        evidence.append({"id": entry[RESOURCE_ID_KEY], "first_sector": first_sector, "sectors": sectors,
                         "table_offset": base, "buffer_sectors": u32(table, BLOCK_MAP_BUFFER_SECTORS),
                         "crc32": f"{crc:08x}", "runs": runs})
    assert zlib.crc32(master) == u32(trailer, TRAILER_MASTER_CRC)
    subprocess.run([*HDIUTIL_VERIFY, str(image)], check=True)
    return {
        "id": image.stem, "mode": profile, "license": "Apache-2.0; synthetic data only",
        "provenance": "Apple hdiutil applied to generated synthetic bytes",
        "os": capture("/usr/bin/sw_vers", "-productVersion"),
        "os_build": capture("/usr/bin/sw_vers", "-buildVersion"),
        "architecture": capture("/usr/bin/uname", "-m"),
        "generator_argv": [arg.replace(str(image.parent) + "/", "") for arg in generate],
        "image_sha256": hashlib.sha256(payload).hexdigest(),
        "raw_sha256": hashlib.sha256(raw).hexdigest(),
        "decoded_sha256": hashlib.sha256(expected_disk).hexdigest(),
        "decoded_crc32": f"{zlib.crc32(expected_disk):08x}",
        "sector_count": u64(trailer, TRAILER_SECTORS), "physical_bytes": len(payload),
        "variant": u32(trailer, TRAILER_VARIANT), "data_fork": [data_start, data_length],
        "data_crc32": f"{u32(trailer, TRAILER_DATA_CRC):08x}", "xml": [xml_start, xml_length],
        "master_crc32": f"{u32(trailer, TRAILER_MASTER_CRC):08x}", "tables": evidence,
        "native_verify_argv": [*HDIUTIL_VERIFY, image.name],
        "native_verify_exit_code": 0,
    }, expected_disk


def make_run_variant(source, raw, name, code):
    """Derive an explicit probe from a tiny native UDRO and verify with Apple independently."""
    original = source.read_bytes()
    trailer = bytearray(original[-TRAILER_BYTES:])
    plist = plistlib.loads(original[len(raw):-TRAILER_BYTES])
    entry = plist[RESOURCE_FORK_KEY][BLOCK_MAPS_KEY][0]
    table = bytearray(entry[RESOURCE_DATA_KEY][:BLOCK_MAP_BYTES])
    disk_sectors = len(raw) // SECTOR_BYTES
    if code == RUN_COMMENT:
        records = [(code, 0, 0, 0, 0), (RUN_RAW, 0, disk_sectors, 0, len(raw))]
        stored = raw
    else:
        data_end = TINY_DATA_START + TINY_DATA_SECTORS
        stored = raw[TINY_DATA_START * SECTOR_BYTES:data_end * SECTOR_BYTES]
        records = [(code, 0, TINY_DATA_START, 0, 0),
                   (RUN_RAW, TINY_DATA_START, TINY_DATA_SECTORS, 0, len(stored)),
                   (code, data_end, disk_sectors - data_end, len(stored), 0)]
    records.append((RUN_END, disk_sectors, 0, len(stored), 0))
    table_crc = zlib.crc32(stored if code == RUN_IGNORE else raw)
    BE_U32.pack_into(table, BLOCK_MAP_CRC, table_crc)
    BE_U32.pack_into(table, BLOCK_MAP_RUN_COUNT, len(records))
    entry[RESOURCE_DATA_KEY] = table + b"".join(
        RUN_RECORD.pack(kind, RUN_NO_COMMENT, start, count, offset, size)
        for kind, start, count, offset, size in records
    )
    xml = plistlib.dumps(plist, sort_keys=False)
    BE_U64.pack_into(trailer, TRAILER_DATA_LENGTH, len(stored))
    BE_U32.pack_into(trailer, TRAILER_DATA_CRC, zlib.crc32(stored))
    BE_U64.pack_into(trailer, TRAILER_XML_OFFSET, len(stored))
    BE_U64.pack_into(trailer, TRAILER_XML_LENGTH, len(xml))
    BE_U32.pack_into(trailer, TRAILER_MASTER_CRC, zlib.crc32(BE_U32.pack(table_crc)))
    image = source.with_name(f"{name}.dmg")
    image.write_bytes(stored + xml + trailer)
    evidence, expected = characterize(image, raw, [], "derived UDRO probe")
    assert expected == raw
    evidence["provenance"] = f"{source.name} with synthetic type {code:#010x} records, independently verified"
    evidence["derived_from"] = source.name
    image.with_suffix(".json").write_text(json.dumps(evidence, indent=2) + "\n")


def make_multi_table(directory):
    synthetic = directory / "synthetic"
    synthetic.mkdir()
    (synthetic / "hello.txt").write_bytes(b"pomologie fixture\n")
    raw = directory / "hybrid.dmg"
    hybrid = [HDIUTIL, "makehybrid", "-hfs", "-hfs-volume-name", VOLUME_NAME,
              "-o", str(raw), str(synthetic)]
    subprocess.run(hybrid, check=True)
    image = directory / f"hybrid-{UDZO}.dmg"
    options = [HDIUTIL, "convert", str(raw), "-format", UDZO, "-o", str(image), "-quiet"]
    subprocess.run(options, check=True)
    evidence, _ = characterize(image, raw.read_bytes(), options, "UDZO with APM")
    evidence["source_generator_argv"] = [arg.replace(str(directory) + "/", "") for arg in hybrid]
    image.with_suffix(".json").write_text(json.dumps(evidence, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=pathlib.Path, help="new directory for fixture output")
    parser.add_argument("--tiny-only", action="store_true")
    args = parser.parse_args()
    args.directory.mkdir()
    all_byte_values = bytes(range(1 << 8))
    cases = {TINY_CASE: bytes(SECTOR_BYTES) + b"A" * SECTOR_BYTES
             + all_byte_values * (SECTOR_BYTES // len(all_byte_values)) + bytes(SECTOR_BYTES)}
    if not args.tiny_only:
        cases["zero"] = bytes(2 * BLOCK_BYTES)
        pattern = b"dmgez synthetic\0"
        cases["mixed"] = (bytes(BLOCK_BYTES) + pattern * (BLOCK_BYTES // len(pattern))
                          + random.Random(MIXED_SEED).randbytes(BLOCK_BYTES) + bytes(BLOCK_BYTES))
    for name, raw in cases.items():
        source = args.directory / f"{name}.raw"
        source.write_bytes(raw)
        for variant in (UDRO, UDZO):
            image = args.directory / f"{name}-{variant}.dmg"
            options = [HDIUTIL, "convert", str(source), "-srcimagekey",
                       RAW_DISK_IMAGE_CLASS, "-format", variant, "-o", str(image), "-quiet"]
            subprocess.run(options, check=True)
            evidence, expected = characterize(image, raw, options, variant)
            assert expected == raw
            image.with_suffix(".json").write_text(json.dumps(evidence, indent=2) + "\n")

    for name, code in (("zero-run", RUN_ZERO), ("ignore-run", RUN_IGNORE),
                       ("comment-run", RUN_COMMENT)):
        make_run_variant(args.directory / f"{TINY_CASE}-{UDRO}.dmg", cases[TINY_CASE], name, code)
    if not args.tiny_only:
        make_multi_table(args.directory)


if __name__ == "__main__":
    main()
