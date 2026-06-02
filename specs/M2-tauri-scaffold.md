# M2-tauri-scaffold — Scaffold tauri-app workspace member

> Milestone: M2 · Issue: #22 · Suggested tier: opus
> Branch: `claude/tauri-scaffold`

## Goal

Add `tauri-app/` to the repository as a Tauri 2 + Vite + React + TypeScript
desktop application. The Rust backend depends on `crates/core`; the webview
frontend renders the note-highway and record screens. This issue delivers only
the scaffold — an empty window that opens — so subsequent issues can build
screens against a real project structure.

## Context

The architecture in `CLAUDE.md` names Tauri as the next frontend after the TUI.
`core` stays pure; `tauri-app/src-tauri` is the integration layer that calls
into `core` and exposes data to the webview via Tauri commands and events.

The design prototypes in `design/*/rockcraft-proto/` are the visual reference
for subsequent screen issues.

## What to do

### Directory layout

```
tauri-app/
  package.json          # workspace root; scripts: dev, build, tauri
  tsconfig.json
  vite.config.ts
  index.html
  src/
    main.tsx            # ReactDOM.createRoot → <App />
    App.tsx             # placeholder: renders "RockCraft" centred on dark bg
    index.css           # reset + CSS custom properties (fonts, palette)
  src-tauri/
    Cargo.toml          # name = "rockcraft-tauri"; depends on core
    tauri.conf.json     # productName = "RockCraft", window 1280×800 min
    src/
      main.rs           # tauri::Builder setup; no commands yet
      lib.rs            # pub fn run() called from main
```

### Cargo workspace

In the root `Cargo.toml`, add `"tauri-app/src-tauri"` to the `[workspace]
members` array so `cargo check --workspace` covers it.

### Frontend dependencies (`package.json`)

```json
{
  "dependencies": {
    "react": "^19",
    "react-dom": "^19"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2",
    "@tauri-apps/api": "^2",
    "@vitejs/plugin-react": "^4",
    "typescript": "^5",
    "vite": "^6"
  }
}
```

### `src-tauri/Cargo.toml`

```toml
[package]
name = "rockcraft-tauri"
version = "0.1.0"
edition = "2021"

[dependencies]
core = { path = "../../crates/core" }
tauri = { version = "2", features = [] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[build-dependencies]
tauri-build = { version = "2", features = [] }
```

### `tauri.conf.json` (key fields)

```json
{
  "productName": "RockCraft",
  "identifier": "dev.rockcraft.app",
  "build": {
    "beforeDevCommand": "npm run dev",
    "devUrl": "http://localhost:1420",
    "beforeBuildCommand": "npm run build",
    "frontendDist": "../dist"
  },
  "app": {
    "windows": [{ "title": "RockCraft", "width": 1280, "height": 800, "minWidth": 960, "minHeight": 600 }]
  }
}
```

### `App.tsx` (placeholder)

```tsx
export default function App() {
  return (
    <div style={{ height: "100vh", display: "flex", alignItems: "center",
                  justifyContent: "center", background: "#0f1016",
                  color: "#e7e8ef", fontFamily: "system-ui, sans-serif",
                  fontSize: 28, fontWeight: 600, letterSpacing: -0.5 }}>
      RockCraft
    </div>
  );
}
```

### CI (`.github/workflows/ci.yml`)

Add a step after the existing Rust gate that runs inside `tauri-app/`:

```yaml
- name: tauri-app ts check
  working-directory: tauri-app
  run: |
    npm ci
    npx tsc --noEmit
```

`cargo check` already covers `src-tauri` via the workspace members addition.
Do **not** run a full Tauri GUI build in CI — `tsc --noEmit` + `cargo check` is
sufficient for the gate.

## Tests

No unit tests required for this scaffold issue. The acceptance criteria below
serve as the test.

## Scope boundaries (do NOT)

- Do not implement any note-highway or record-screen UI (separate issues).
- Do not add Tauri commands or events yet.
- Do not port `highway.js` or any canvas engine.
- Do not add fonts or icon assets — system-ui is fine for the placeholder.
- Do not run `cargo clippy` on `tauri-app/src-tauri` in CI yet (Tauri codegen
  triggers false positives until commands are defined).

## Acceptance

- [ ] `tauri-app/` exists with the layout above
- [ ] `cargo check --workspace` includes `rockcraft-tauri` and passes
- [ ] `npx tsc --noEmit` inside `tauri-app/` passes
- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR opened against `main` from `claude/tauri-scaffold`, `Closes #22`
