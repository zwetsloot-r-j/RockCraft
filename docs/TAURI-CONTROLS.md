# Tauri controls audit (M7-tauri-L · #188)

The map of every interactive control on the Tauri screens and its resolved
state. A control is **real** only when a `core::Action` (or a host IPC command)
backs it; otherwise it is rendered **disabled** with a `title` tooltip, or
**removed** if it carried no meaning at all. No control may silently no-op.

Keep this current when a screen gains or wires a control.

## Record (`screens/record/`)

| Control | Where | Status | Keybinding | Notes |
|---|---|---|---|---|
| Record ● | RecordToolbar | **real** | `r` | `record_start` / `record_stop` |
| Stop ■ | RecordToolbar | **real** | `r` (toggle) | `record_stop` |
| Save | RecordScreen | **real** | `s` | `record_save` |
| Choose / change backing | RecordHeader | **real** | — | native dialog → `record_start(backing)` |
| Level meter | RecordHeader | **real** (cosmetic ok) | — | velocity-derived from the engine |
| MIDI device chip | RecordHeader | **real** | — | live `midi_status` poll |
| Dirty-exit prompt | RecordScreen | **real** (added here) | `Esc` → `s`/`d`/`Esc` | Save / Discard / Cancel; mirrors `crates/tui/src/record.rs` |
| Rewind / Play take / Loop | RecordToolbar | disabled | — | no take-playback transport yet |
| Trim / Quantize / Punch-in | RecordToolbar | disabled | — | no core action |
| Delete / Undo / Redo | RecordToolbar | disabled | — | no core action |
| Snap (1/8…1/32) | RecordToolbar | disabled | — | until a quantize action exists |
| Metronome toggle | RecordHeader | disabled | — | `toggle_metronome` exists but no path into the live record session |
| Count-in toggle | RecordHeader | disabled | — | `start_count_in_record` exists but no path into the live record session |
| Nudge / Snap (note) | NoteInspector | disabled | — | no per-note edit action |
| Clef (Grand/Treble) | RecordToolbar | **removed** | — | changed no render — pure decoration |
| Spelling (♯/♭) | RecordToolbar | **removed** | — | changed no render — pure decoration |

The dirty-exit decision logic is the pure `nextExit()` helper in
`screens/record/exit.ts` (kept obviously correct without a JS test framework, per
the M7 convention). It is dispatched from a **capture-phase** key listener so it
intercepts `Esc` before the shell's global `Esc`→menu handler
(`shell/Router.tsx`).

## Play / highway (`screens/highway/`)

| Control | Where | Status | Keybinding | Notes |
|---|---|---|---|---|
| Hear-song `♪ m` | HighwayHeader | **real** | `m` | `play_toggle_hear_song`; chip reflects state |
| Wait `⏸ w` | HighwayHeader | **real** | `w` | `play_set_wait`; chip reflects state |

These are status indicators, not buttons — they reflect the keyboard toggles.
No no-op controls.

## Menu (`screens/menu/`)

Every item routes to a real action (`navigate` / `latestRecording` /
`importStart` / `openVideoFilePicker` / window close). "Import from URL…" is
hidden unless a fetch command is configured (`import_url_available`). No no-op
controls.

## Edit / compose media (`screens/edit/`)

Backing audio and the background video are **independent** piece attributes;
swapping or detaching one never disturbs the other (M10-E).

| Control | Keybinding | Status | Notes |
|---|---|---|---|
| Choose / **replace** backing | `B` | **real** | native audio picker → `edit_set_backing`; swaps the file in place, leaving the video backdrop visible/playing |
| Detach backing | `Backspace` / `Delete` | **real** | `edit_clear_backing`; clears only the audio — the video remains |
| Backing align nudge | `,` `.` `;` `'` | **real** | `nudge_backing_offset` (±10 ms / ±250 ms) |
| Attach / detach video backdrop | `V` | **real** | native video picker → `edit_set_video` / `edit_clear_video` |
| Video align nudge | `,` `.` `;` `'` (while attached) | **real** | `edit_set_video_offset` (diverts the nudge keys while a backdrop is present) |

## Out of scope

The rest of the edit/compose grid (`screens/edit/`) is audited under
M7-tauri-M / M7-tauri-N, not here.
