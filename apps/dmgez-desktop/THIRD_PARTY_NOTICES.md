# Native desktop dependency notices

Include this file and the accompanying [licenses](licenses/) directory with desktop binary distributions. These notices cover the embedded fonts and graphics loader reviewed for the initial desktop dependency set.

## Embedded fonts

The desktop enables `eframe/default_fonts`, which selects `epaint_default_fonts 0.37.0`. The font set is the same for the checked macOS, Linux, and Windows target graphs.

| Embedded input | Shipped notice |
| --- | --- |
| Hack Regular | [Hack MIT and Bitstream Vera notices](licenses/Hack-Regular.txt). The upstream notice also records DejaVu's public-domain contributions. |
| Ubuntu Light | [Ubuntu Font Licence 1.0](licenses/Ubuntu-Light-UFL.txt). The embedded font records “Copyright 2011 Canonical Ltd. Licensed under the Ubuntu Font Licence 1.0”. |
| egui icons | [John Slegers's MIT notice](licenses/egui-icons-MIT.txt). This five-glyph subset comes from `emoji-icon-font.ttf`. |

The `monochrome_emoji_fonts` feature is disabled for these target graphs. Its optional Noto Emoji and full emoji-icon font files are present in the upstream Cargo source package. Both were absent from the checked macOS debug binary. The package's SPDX declaration includes SIL OFL 1.1 for its source contents.

[Pinned font definitions and notices](https://github.com/emilk/egui/tree/7c0b40402271e8d40cda69f87cf3c2617c2e4382/crates/epaint_default_fonts)

## Graphics loader

`libloading 0.8.9` uses the [ISC license](licenses/libloading-ISC.txt). The desktop resolves this version through `glutin 0.32.3` on the checked macOS, Linux, and Windows targets. The Linux graph also reaches it through `dlib` and `x11rb`. The inspected macOS debug binary used the CGL backend and its symbol table contained no `libloading` symbol names.

[Pinned loader license](https://github.com/nagisa/rust_libloading/blob/f4ec9e702de2d0778bccff8525dc44e4cacac2d1/LICENSE)

## Dependency policy review

These version-scoped cargo-deny entries cover the additional licenses declared by the two selected packages.

```toml
{ crate = "epaint_default_fonts@=0.37.0", allow = ["OFL-1.1", "Ubuntu-font-1.0"] },
{ crate = "libloading@=0.8.9", allow = ["ISC"] },
```

The [desktop smoke check](tests/check_desktop.py) verifies the target-specific dependency graphs, the shipped notice bytes, the GUI tests, and isolation from the default CLI graph. Supply `--binary` with a built desktop executable to check its embedded font bytes.
