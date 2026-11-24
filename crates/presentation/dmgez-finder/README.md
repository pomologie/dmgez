# Finder icon-view store

`write_store` produces a deterministic `.DS_Store` for a folder in a newly built image. It takes the folder's settings and the final names and positions of its direct children. The standard records capture window bounds and chrome, icon-view preferences, default view, and icon positions. Window origins follow dmgbuild's screen coordinates with y increasing upwards. Icon positions are unsigned whole points with y increasing downwards. The color profile accepts `#RGB` and `#RRGGBB`.

The initial writer accepts printable ASCII item names. Other views, image aliases, template backgrounds, and editing existing stores require additional qualification. The caller resolves folder inheritance and reserves the generated store before planning catalog IDs.

## Evidence

The fixture checks the layout-record portion of D-FINDER-01-T01. It covers deterministic encoding, binary record tags, window geometry, separate preview and item-info settings, exact icon centers, short hexadecimal color, validation failures, and the page boundary. The native Finder visual gate remains open.

The window and icon record choices follow dmgbuild 2.0. [Settings](https://dmgbuild.readthedocs.io/en/stable/settings/) · [Record emission](https://github.com/dmgbuild/dmgbuild/blob/v2.0.0/src/dmgbuild/core.py)

The pinned writer bundles the reader and alias codec. The package exposes no separate writer feature. [ds_parser 0.4.0](https://docs.rs/ds_parser/0.4.0/ds_parser/)

The writer is limited to one 4096-byte B-tree leaf. In a scratch test, `ds_parser` 0.4.0 wrote 200 synthetic icon locations and read back 200 without warnings. The independent reader found 202 records for those same 200 keys, including two duplicated separator records. The adapter measures every record before writing and rejects layouts that exceed one leaf. [ds_store 1.3.4](https://ds-store.readthedocs.io/en/latest/)

```sh
python3 -m venv /tmp/dmgez-finder-oracle
/tmp/dmgez-finder-oracle/bin/python -m pip install ds_store==1.3.4 mac_alias==2.2.3
PATH="/tmp/dmgez-finder-oracle/bin:$PATH" cargo test --locked -p dmgez-finder -- --include-ignored
```

`ds_parser` 0.4.0 and its transitive dependency `alias_record` 0.3.0 use the Blue Oak Model License 1.0.0. [License text](https://blueoakcouncil.org/license/1.0.0)
