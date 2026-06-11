# M7-tauri-0-solid-swap — Swap frontend framework React → SolidJS

> Milestone: M7 · Issue: #171 · Suggested tier: sonnet
> Branch: `claude/tauri-solid-swap`
> Depends on: M2-tauri-scaffold (#22, merged)
> **Blocks every frontend issue**: #23, #24, #161 (frontend half), #162 and downstream — land this first.

## Goal

Replace React with SolidJS in `tauri-app/` while it is still a placeholder, so
no screen is ever written twice. Decision rationale: Solid's fine-grained
reactivity fits the high-frequency chrome (score/combo headers, status bars
re-rendering on every `snapshot` event) without vDOM diffing, and it keeps
JSX, so the React-flavoured design prototypes in
`design/*/rockcraft-proto/*.jsx` still translate near-mechanically. The
canvas engines (`HighwayCanvas`, `RecordCanvas`, `EditCanvas`) are
framework-free TS classes and are unaffected.

## Context

- `tauri-app/` from #22: Vite + React 19 + TS strict; `App.tsx` renders the
  "RockCraft" placeholder. No screens exist yet.
- CI runs `npx tsc --noEmit` in `tauri-app/` — that step stays, unchanged.
- Nothing in `crates/` or `src-tauri/` is touched by this swap.

## What to do

### `package.json`

- Remove: `react`, `react-dom`, `@types/react`, `@types/react-dom`,
  `@vitejs/plugin-react`.
- Add: `solid-js` (^1.9), dev: `vite-plugin-solid` (^2.11).

### `vite.config.ts`

```ts
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
export default defineConfig({ plugins: [solid()], /* keep existing tauri server opts */ });
```

### `tsconfig.json`

```jsonc
"jsx": "preserve",
"jsxImportSource": "solid-js",
```

(remove `"jsx": "react-jsx"`; keep strict mode and everything else.)

### `src/main.tsx`

```tsx
import { render } from "solid-js/web";
import App from "./App";
render(() => <App />, document.getElementById("root")!);
```

### `src/App.tsx`

Same placeholder, Solid-flavoured (plain JSX, no hooks). Visual output
identical: "RockCraft" centred on the dark background.

### Solid conventions (referenced by all later frontend specs)

Add `tauri-app/CONVENTIONS.md` with exactly these rules:

1. **Components run once.** No re-render mental model; reactivity lives in
   signals. Never destructure props (breaks reactivity); use `props.x` or
   `splitProps`.
2. **State**: `createSignal` for scalars, `createStore` for object state
   (e.g. the `ComposerSnapshot` mirror in #161). No external state libraries.
3. **Lifecycle**: `onMount` / `onCleanup` for engine start/stop, event
   listeners, intervals — every `listen()` from Tauri gets its unlisten in
   `onCleanup`.
4. **Refs**: `let el!: HTMLCanvasElement;` + `ref={el}`.
5. **Control flow**: `<Show>` / `<For>` / `<Switch>` instead of ternaries
   and `.map` for lists/conditional screens.
6. **Styles**: inline style objects use kebab-case string keys
   (`"flex-direction": "column"`), unlike React's camelCase.
7. **Porting the design prototypes**: `useState` → `createSignal`,
   `useRef` → plain `let` + `ref`, `useEffect(…, [])` → `onMount`/
   `onCleanup`, `useReducer`-as-tick → a `frame` signal bumped by the
   throttle loop; values read in JSX become accessor calls (`metro()`).

## Tests

No new automated tests. `npx tsc --noEmit` must pass with the Solid JSX
types; the Rust gate is untouched.

## Scope boundaries (do NOT)

- Do not add screens, routing, or `@solidjs/router` — placeholder only.
- Do not touch `crates/` or `tauri-app/src-tauri/`.
- Do not change CI beyond nothing-at-all (the tsc step already works).
- Do not reformat unrelated scaffold files.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] `npx tsc --noEmit` passes in `tauri-app/`
- [ ] `package.json` has no `react*` entries; `npm run dev` shows the same
      placeholder window
- [ ] `tauri-app/CONVENTIONS.md` present with the rules above
- [ ] PR opened against `main` from `claude/tauri-solid-swap`, `Closes #171`
