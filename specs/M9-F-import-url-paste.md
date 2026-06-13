# M9-F — Allow pasting a URL into the "Import from URL" dialog (Tauri)

> Milestone: M9 — Tauri UX consolidation · Issue: #205 · Suggested tier: cheap
> Branch: `claude/m9-import-url-paste`

## Goal

The Tauri "Import from URL…" dialog cannot accept a pasted URL — the user must
type the whole thing by hand. Make paste (and normal text editing) work.

## Context

- `tauri-app/src/screens/import/UrlInput.tsx` does **not** use a real text input.
  It builds the URL string from individual `keydown` events and **explicitly
  excludes modifiers**:

  ```ts
  if (e.key.length === 1 && !e.ctrlKey && !e.altKey && !e.metaKey) { … }
  ```

  So `Ctrl/Cmd+V` is filtered out and there is no `paste` handler — pasting is
  impossible, and the synthetic editor also misses normal niceties (caret,
  selection, mid-string editing).
- The component is a centered modal; `Enter` submits, `Esc` cancels (back to
  menu). It calls `props.onSubmit(url)` which feeds `importStart` (see
  `MenuScreen.tsx`). The downstream import pipeline is unchanged by this task.

## What to do

- Replace the synthetic string-builder with a **real focusable `<input>`** (or
  `<textarea>`), auto-focused on mount, so the OS/webview handle paste, caret,
  selection, and editing natively. Keep the modal styling/visuals.
- Preserve behaviour: trim on submit; reject empty (`"URL must not be empty"`);
  `Enter` submits; `Esc` cancels back to the menu. Ensure the global router `Esc`
  and the `Enter`-to-submit still work with the input focused, and that typing in
  the field does **not** leak to the shell's mock-MIDI handler.
- If the webview blocks clipboard paste by default, enable it for this input
  (Tauri webview clipboard / allow paste on the field) so `Ctrl/Cmd+V` works.

## Tests

- A component test: a `paste` event (or setting the input value) populates the URL
  and `Enter` calls `onSubmit` with the pasted value; empty submit shows the error;
  `Esc` navigates to the menu.

## Scope boundaries (do NOT)

- Do **not** change the import pipeline, `importStart`, or `import_url_available`.
- TUI URL input (`crates/tui/src/import_screen.rs`) is a terminal field with
  different paste semantics — **out of scope** here (note it in the PR if you spot
  a parallel gap).
- Do **not** restyle the dialog beyond swapping in the real input.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] A URL can be pasted (Ctrl/Cmd+V) and edited in the Tauri import dialog, then
      submitted
- [ ] PR opened against `main` from the branch above, `Closes #205`
