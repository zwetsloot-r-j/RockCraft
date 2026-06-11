# M7-tauri-B-shell-menu — App shell: screen router + main menu

> Milestone: M7 · Issue: #162 · Suggested tier: sonnet
> Branch: `claude/tauri-shell-menu`
> Depends on: M2-tauri-scaffold (#22, merged) · coordinates with #23/#24

## Goal

Turn the placeholder `App.tsx` into the application shell: a screen router
mirroring the TUI's `Screen` enum, with the main menu as home. Later issues
slot their screens into this router instead of inventing navigation.

## Context

- TUI reference: `crates/tui/src/app.rs` — `Screen` enum (`Menu`, `Record`,
  `Play`, `Edit`, `BackingPicker`, `VideoPicker`, `UrlInput`, `Importing`,
  `Library`) and `menu_activate()` for the item → screen mapping.
- Menu items (exact labels and order, from `app.rs`): **Record**, **Play last
  recording**, **Compose (new)**, **Edit last recording**, **Choose backing
  track**, **Import from video file…**, **Import from URL…** (conditional —
  see below), **Quit**.
- The Highway (#23) and Record (#24) screens may land before or after this;
  whichever is present gets a route, the rest render placeholders.

## What to do

### `src/shell/`

```
shell/
  Router.tsx     # const [screen, setScreen] = useState<Screen>({kind:"menu"})
  screens.ts     # type Screen = { kind: "menu" } | { kind: "record" } |
                 #   { kind: "play" } | { kind: "edit" } |
                 #   { kind: "backing-picker" } | { kind: "video-picker" } |
                 #   { kind: "url-input" } | { kind: "importing" } |
                 #   { kind: "library" }
                 # (later issues extend variants with payloads, e.g. bundle dir)
  Placeholder.tsx # full-window dark panel: screen name + "not ported yet" +
                 # "Esc to menu"
```

`Router.tsx` provides `{ screen, navigate }` via React context. Global
`keydown` listener: `Escape` navigates to the menu from any non-menu screen
(menu itself: Escape does nothing — Quit is an explicit item, unlike the
TUI's `q`, because accidental window-close is worse on desktop).

### `src/screens/menu/MenuScreen.tsx`

Dark full-window menu in the prototype style (`#0f1016` background,
`Space Grotesk` if the font link exists, RockCraft wordmark above the list).
Items as a vertical list with a highlighted selection bar. Keyboard:
ArrowUp/ArrowDown/`j`/`k` move (wrapping), Enter activates. Mouse: hover
selects, click activates.

Item behaviours for now:

- Record / Play last recording / Compose (new) / Edit last recording /
  Choose backing track / both Import items → `navigate` to the matching
  screen variant (mostly placeholders until #163–#170).
- **Import from URL…** renders, but greyed-out with title
  `"no fetch command configured"` — live detection arrives with #170.
- **Quit** → `getCurrentWindow().close()` from `@tauri-apps/api/window`.

### `App.tsx`

Render the router. If `screens/highway/` (#23) or `screens/record/` (#24)
already exist on the branch, route `play` / `record` to them; otherwise to
`Placeholder`.

## Tests

No automated UI tests required. `npx tsc --noEmit` must pass; manual
acceptance below.

## Scope boundaries (do NOT)

- Do not implement any destination screen (library, edit, import, …).
- Do not add a routing library — `useState` + context only.
- Do not modify anything in `crates/`.
- Do not call IPC commands (the menu is purely frontend in this issue).

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] `cargo test --workspace` green
- [ ] `npx tsc --noEmit` passes
- [ ] `npm run dev`: menu shows all items in TUI order; arrows/`j`/`k`/Enter
      navigate; Esc returns from a placeholder to the menu; Quit closes
- [ ] PR opened against `main` from `claude/tauri-shell-menu`, `Closes #162`
