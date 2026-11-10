# 0092 — Static text label widget

- **Status:** accepted
- **Date:** 2026-11-10
- **Affects:** `libs/zc-abi`, F8e
- **Supersedes:** none (extends ADR 0087)

## Context

Phase F8e aims to transition the widget layer from a pair of hard-coded
terminal buttons (`Clicks: N` and `Reset`) toward a composable widget set.
The `WidgetKind::Label` enum variant was introduced in ADR 0087 but had no
spec builder, placement, navigation rules, or rasterizer.

Unlike interactive buttons, static labels display informative or status text,
do not maintain click counters, must not capture pointer clicks or keyboard
focus, and do not draw button border rings or focus rectangles.

## Decision

**Add static text label widgets with spec-based layout, hit-testing, focus
skipping, and glyph rendering.**

- `WidgetSpec` expresses `WidgetKind` (`Button` or `Label`) and `width`.
  `strip_specs` computes widget positions along the strip; the legacy `strip`
  remains as an exact wrapper constructing button specs.
- `Layout::hit_test(x, y)` resolves a window-local coordinate to the placed
  widget, and `Layout::kind_of(id)` returns its kind.
- `Focus::advance_in(&Layout)` and `Focus::take_in(id, &Layout)` ensure focus
  cycles exclusively across interactive widgets (`WidgetKind::Button`), skipping
  static labels.
- `label_pixel(font, text, w, bg, fg, x, y)` rasterizes centered label text
  without border decorations.
- `Term::click` uses `Layout::hit_test` and `Focus::take_in` to ignore clicks
  on non-interactive widgets.

## Consequences

- Applications and window strips can now place non-interactive text labels
  alongside buttons.
- Tab navigation cleanly skips labels without stuttering or showing phantom
  focus rings.
- All 3 proof checksums (`0x18f9ce724a929efc`, `0x6d66e82d0f2d6fb8`,
  `0xf89e1ca133f3e8dc`) remain byte-identical and verified.

## References

- [ADR 0087](0087-a-windows-widgets-are-data-one-layout-pass-places.md)
- [ADR 0089](0089-a-click-gives-a-widget-the-focus.md)
- [ADR 0091](0091-space-activates-the-focused-widget.md)
