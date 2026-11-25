# Project edit verification

[The D-V11 state machine](project_edit_state_machine.rs) compares the public
`ProjectDocument` reducer with an independent semantic model after each
generated action. It explores up to 20 actions over four destinations,
including grouped swaps, append and remove, layout changes, invalid batches,
no-op edits, undo, and redo.

The model checks ordered path and payload identity, position, visibility,
extension flags, image label and output. It checks exact source and revision
restoration, including redo branches after invalid or no-op edits. The
fixtures include table arrays and inline declarations, LF and CRLF, and
sources with and without a trailing newline. Each revision is reparsed
to compare serialized settings with the semantic model.

```sh
# From the dmgez checkout root
PROPTEST_CASES=1024 PROPTEST_RNG_SEED=18532457 cargo nextest run --locked \
  -p dmgez-config --features project-edit --test project_edit_state_machine
```

These checks cover bounded in-memory command history for D-GUI-01-T01/T02.
History eviction, GUI interaction, and preview behavior have separate test
scopes under D-V11.
