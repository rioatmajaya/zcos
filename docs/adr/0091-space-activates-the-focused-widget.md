# 0091 — Space activates the focused widget at a bare prompt

- **Status:** accepted
- **Date:** 2026-11-10
- **Affects:** `libs/zc-abi`, F8
- **Supersedes:** none (extends ADR 0089)

## Context

ADR 0089 gave the widget strip a `Focus`: an accepted click focuses its
widget, `Tab` advances focus around the placed widgets, and the focused
widget wears `FOCUS_EDGE` on its border. What remained open was the action
half of keyboard focus — a user could cycle the ring with `Tab`, but still
had to reach for the pointer to press the focused button or reset target.

In a terminal window, `Enter` belongs to submitting a command line and
`Space` separates command arguments (`cat hello.txt`, `echo hi`). Routing
`Space` unconditionally to the focused widget would swallow argument spaces
whenever a previous click or `Tab` left a widget focused.

## Decision

**`Space` activates the focused widget when the input line is at a bare
prompt, and `Escape` clears the widget focus.**

- `Term::activate(id)` is the single per-widget action path (`BUTTON_ID`
  increments the counter, `RESET_ID` zeroes it), shared by `Term::click` and
  `Term::key`.
- In `Term::key`, when `ch == b' '`, the current line holds only the prompt
  (`at_bare_prompt`), and `Focus::id()` names a placed widget, `Space`
  activates that widget and never reaches the transcript. Once any character
  has been typed past the prompt, `Space` goes to `push_key` unchanged so
  command arguments work whether or not a widget holds the ring.
- `Escape` (`0x1b`) calls `Focus::clear()`, dropping the ring back to `None`
  without touching the transcript.

## Consequences

- Both strip widgets (`Clicks: N` and `Reset`) can be focused with `Tab` and
  pressed with `Space` from the keyboard alone.
- The scripted boot proof runs its keystrokes before the pointer script
  focuses a widget, and its `cat hello.txt` space is mid-command anyway, so
  all three locked proof checksums (`0x18f9ce724a929efc`,
  `0x6d66e82d0f2d6fb8`, `0xf89e1ca133f3e8dc`) stay byte-identical.

## References

- [ADR 0089](0089-a-click-gives-a-widget-the-focus.md)
- [ADR 0088](0088-a-second-widget-resets-the-first.md)
