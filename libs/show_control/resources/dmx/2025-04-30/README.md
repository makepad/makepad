# Recovered handmade DMX scenes

These 16 numbered files are byte-for-byte copies of
`local/dmx_recovered/makepad-backup_local_dmx/dmx0.ron` through `dmx15.ron`, recovered from
`makepad-backup/local/dmx` and duplicated identically in
`makepad_broken2/local/dmx`. Their sizes and SHA-256 hashes were checked against
`local/dmx_recovered/index.tsv` (124 files in nine directories).

The bank is selected by its current-state snapshot date, **2025-04-30**, the
second distinct snapshot date before `makepad7/dmx.ron` on 2025-05-03. The
numbered files themselves have 2025-04-11 timestamps. The recovered `dmx.ron`
current state is deliberately excluded; selecting this bank does not load it.
Duplicate scenes (including 8–11) retain their original slots.

`dmxN.ron` supplies zero-based slot N, displayed as P(N+1). Slots 0–12 use
`ControllerState` directly. Slots 13–15 use the older `dial_a/b/c` layout,
translated according to the January 14, 2025 hardware migration in
`683bbf60654836458bfe8c1eb3a5950718393179`, `experiments/huedmx/src/app.rs`:

| Legacy control | Modern control |
| --- | --- |
| `fade[1]`, `fade[2]` | `fade[2]`, `fade[1]` (outer/inner movers) |
| `fade[6]`, `fade[7]` | `fade[7]`, `fade[6]` (UV) |
| `dial_a[0..=5]` | `dial_1[0..=5]` |
| `dial_c[0, 2, 1, 3]` | `dial_top[0, 1, 2, 3]` |
| `dial_b[1]` | `dial_top[5]` |
| `dial_b[0, 2, 3, 4]` | `dial_5[0..4]` |
| `dial_b[5, 6, 7]` | `dial_0[0..3]` |
| `dial_a[6]`, `dial_a[7]` | `tempo`, only when the two speeds agree |

Both legacy speed values are zero in all three selected files. Conflicting
speeds in a legacy override are rejected. Other faders retain their values;
unused legacy `dial_c[4..8]` have no active destination in the migration.
Unused modern controls default to zero. Scene loading still preserves the
desk's existing entire `dial_0` smoke-timing bank after conversion.

Missing `preset_NN.ron` slots fall back to these compiled-in originals.
Saves use the separate checkout-local `local/vj/dmx/2025-04-30` overlay,
resolved from the crate's build-time absolute path, or `VJ_DMX_PRESET_DIR`
when set. Existing invalid/unreadable overrides fail instead of falling back.
Only an overlay's `current.ron` can restore current state. The recovery
originals and `examples/automate/local/dmx` are not save destinations by default.

Hardware remains note 52/channels 0–7 → P1–P8 and notes 82–86 → P9–P13.
P14–P16 are on-screen only; clip-grid notes 0–39 and CC 15 remain reserved to VJ.
