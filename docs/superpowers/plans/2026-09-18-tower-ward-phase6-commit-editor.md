# Phase 6 — In-app commit editor

Date: 2026-09-18. The spec's deferred "1b fast-follow": `c` opens an
in-app multi-line commit editor instead of shelling out. The external
editor stays available on `C`.

## Design decisions

- **New `text_area.rs` component.** GPUI 0.2.2 ships no multi-line
  editor, so the app gets one built on the same primitives as
  `text_field.rs`: one String + a byte-offset cursor, raw key events,
  `key_char` for typed characters. Multi-line ops: enter inserts `\n`;
  up/down keep the column; left/right and backspace/delete cross line
  boundaries; home/end are line-scoped. No wrapping in v1 (horizontal
  overflow scrolls); no IME (standing GPUI 0.2.2 limitation, same as
  the single-line field).
- **`c` = in-app editor, `C` = external `$EDITOR`.** Both flows share
  the commit core. The dialog model removes the old escape-hatch
  coupling: the modal's esc closes the editor (nothing to abandon), so
  `commit_editor_active` machinery stays only on the `C` path.
- **Messages are comment-stripped like git's.** The draft text goes
  through `commit::commit_from_draft`, which reads `core.commentChar`
  (default `#`), strips comment lines, and refuses an empty result.
  The dialog pre-fills commented hints (subject guidance + staged
  count) that disappear from the final message.
- **Confirm is cmd+enter / ctrl+enter or the Commit button** — enter
  must remain a newline inside a multi-line editor.

## Tasks

1. **`text_area.rs`** — multi-line entity (cursor movement, edits,
   click-to-focus, `|` cursor in the active line, vertical scroll).
   Unit-level behavior covered through app tests.
2. **Engine — `commit_from_draft`.** Comment-strip + empty refusal +
   `commit()`.
3. **Store — `commit_in_app`.** Same gates and completion flags as the
   editor flow (mutated + history_changed + refresh).
4. **Dialog — `CommitEditor`.** Card with the area, live staged count,
   hint pre-fill, confirm button (disabled while empty).
5. **Keys.** `c` opens the dialog; `C` keeps the external flow. Footer,
   README, version 0.7.0, ledger, full gates.
