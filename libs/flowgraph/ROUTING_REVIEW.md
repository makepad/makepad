F19 routing review — 2026-09-04
The exact far-dragged screenshot coordinates already pass on the starting code;
they do not reproduce the reported screen-sized loop. Sizes are approximate; unspecified obstacle sizes use 290×200 (picture: 290×150).
The saved prompt position does reproduce an unnecessary trip above both cards:
202.066 units versus Manhattan 118, at prompt (-1359, -426).
Cause: valid_orthogonal required every interior segment to span two full fillets.
It rejected the direct four-unit vertical run, although rounded_samples already
clamps each fillet to half each adjoining segment. Removing that restriction
retains minimum endpoint stubs and both rendered-path clearance checks.
Routed mode enumerates a forward column and obstacle-boundary rows; the narrow
tier also tries one extra column. It ranks bends, then length, deterministically.
F10 keeps 12-unit clearance / 16-unit fillets unless an eligible narrow channel
wins (6 + half-spacing centre clearance / 8-unit fillets). F15 permits the
adjacent vertical run beside an owner's 6-unit envelope; this fix preserves it.
Sticky routing keeps a broad choice within 5% of the selected candidate's length.
Bezier mode uses only directional port controls and deliberately ignores cards.
Canvas caches corridor obstacles per edge; auto-flip/preview route with all cards.
CROSSING/BEND/LOOP costs score both auto-flip alternatives, not router candidates.
Suspects: (a) inflated owner guides can contain a stub and reject the forward
column, but endpoint-aware boundary rows handle the reported final geometry;
(b) F15's inset is symmetric and rendered clearance remains checked;
(c) fallback minimizes collisions, then bends/length, never maximizes length;
(d) LOOP_COST is not selectively applied to direct routes; (e) no below/above
sign error reproduced. The full-radius segment rejection caused the saved detour.
Added tests in tests/wire_route.rs: screenshot_prompt_below_left_of_expand_does_not_wrap,
screenshot_mirrored_prompt_above_left_of_expand_does_not_wrap,
screenshot_flipped_expand_uses_only_its_near_corner,
screenshot_expand_to_add_style_stays_between_its_ports,
screenshot_drag_from_saved_position_never_takes_an_outside_row (457 positions × 2 anchors),
nearly_level_ports_keep_short_fillets_and_card_clearance (both y signs and bundle offsets).
No existing expectations changed. The first three static fixtures passed before the fix.
Remaining risks and proposed fixtures (not implemented; require broader routing changes):
- Far obstacles omitted from the corridor can obstruct a detour: detour_hits_remote_card;
  ports (0,0)→(400,0), wall (100,-200,200,400), omitted card (50,-260,300,30).
- Bend-first ranking and narrow-tier eligibility can favor a long outer row over a short
  dogleg: short_zigzag_beats_outer_row, with staggered tall blockers and a clear central lane.
- Straight/collinear candidates do not enforce port sides; aligned_flipped_ports_keep_tangents
  should cover vertical ports and horizontal same-side ports, including fallback reversals.
- Overlapping cards or blocked stubs can exhaust this bounded search; blocked_stub_penetration
  should extend dense_blocking_stubs_falls_back_to_orthogonal with collision/length assertions.
