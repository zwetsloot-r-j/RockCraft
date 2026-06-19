# Running the desktop app on a Windows host & driving it from WSL

> **Who this is for:** an AI agent (or human) whose repo lives in **WSL**
> (`/mnt/e/projects/RockCraft`) but whose **GUI runs on the Windows host**
> (`E:\projects\RockCraft`). The Tauri desktop app (`rockcraft-tauri`) needs a
> real display + WebView2, so it runs on Windows; the control socket can be
> driven from either side. This page is the runbook for the
> [`BACKING-MOVIE-SCENARIO`](BACKING-MOVIE-SCENARIO.md) and any other
> agent-control session against the desktop app.
>
> If you're on native Linux/macOS or a headless CI box, none of the Windows/WSL
> bridging below applies — build and run normally (the example launches the app
> itself, under `xvfb-run` when headless).

## TL;DR

```bash
# 1. Build the frontend on Windows (node_modules must be Windows-native, see below)
cmd.exe /c "cd /d E:\projects\RockCraft\tauri-app && npm install && npx vite build"

# 2. Build the app WITH the custom-protocol feature (see gotcha #1)
/mnt/c/Users/<you>/.cargo/bin/cargo.exe build -p rockcraft-tauri \
    --features tauri/custom-protocol --target-dir target-win

# 3. Run it on the host with the control socket pinned, cwd = repo root
cmd.exe /c "cd /d E:\projects\RockCraft && set ROCKCRAFT_CONTROL_ADDR=127.0.0.1:9001&& \
    target-win\debug\rockcraft-tauri.exe --control"

# 4. Drive it from WSL (proxies cleared; sandbox must allow loopback — see gotcha #3)
node scripts/drive-backing-movie.mjs        # attaches to the running app, no app-spawn
```

The committed Rust twin — `cargo run -p rockcraft-control --example
backing_movie_session` — **spawns its own** app instance, so it's simplest to run
**entirely on Windows** (set `ROCKCRAFT_TAURI_BIN` to the `target-win` exe). The
`scripts/drive-backing-movie.mjs` driver is the WSL-side counterpart that
*attaches* to an already-running host app instead of spawning one.

---

## Gotcha #1 — "localhost connection refused" / blank Edge window

**Symptom:** the app window opens but shows *"this page can't be reached —
localhost connection refused"* with a Microsoft Edge logo.

**Why:** the Edge chrome is normal — Tauri uses **WebView2 (Chromium Edge)** on
Windows. The error is that the webview is pointed at the Vite **dev server**
(`devUrl http://localhost:1420`), which isn't running. Tauri's build script sets
`dev = !custom_protocol`; a plain `cargo build` (no feature) compiles in **dev
mode**, so the webview loads `devUrl` instead of the embedded `dist/`.

**Fix:** build with **`--features tauri/custom-protocol`** (what `cargo tauri
build` enables automatically; a raw `cargo build` does not, and this app declares
no `custom-protocol` feature of its own). The frontend `dist/` must also exist —
run `npx vite build` first.

- `node_modules` installed under WSL contains **Linux** binaries; `tsc`/`vite`
  then fail on Windows with "not recognized as a command". Re-run `npm install`
  under Windows (`cmd.exe /c "cd /d … && npm install"`) before `npx vite build`.
  (`npm run build` also runs `tsc --noEmit`, which may not be on PATH; `npx vite
  build` alone produces `dist/`.)
- Linking can fail with **"access denied (os error 5)"** if a previous app
  instance still holds the `.exe` — `taskkill /F /IM rockcraft-tauri.exe /T`
  first.

## Gotcha #2 — choosing the working directory

`save_bundle {kind: quick_save}` writes to **`recordings/take-…` relative to the
app's cwd**. Launch the app with **cwd = repo root** (`cd /d E:\projects\RockCraft`)
so the bundle lands at `/mnt/e/projects/RockCraft/recordings/…`, visible from WSL
for any file assertions. The app keeps composer state for its whole lifetime, so
**restart it between scenario runs** or authored notes accumulate and
`notes.len() == 4`-style checks fail.

## Gotcha #3 — connecting from WSL to the Windows app

This repo's WSL2 uses **mirrored networking** (eth0 on the LAN subnet, no
`vEthernet (WSL)` adapter), so WSL **can** reach the Windows app at
`ws://127.0.0.1:9001` — correcting the old assumption that "WSL 127.0.0.1 ≠
Windows loopback". Two things still block it:

1. **Proxy env vars.** `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` (socks5h) are
   set in this environment; Node's built-in global `WebSocket` (undici, Node ≥21)
   routes through them and the connection fails. Clear them for the driver:
   ```bash
   env -u HTTP_PROXY -u HTTPS_PROXY -u ALL_PROXY -u http_proxy -u https_proxy \
       -u all_proxy -u GRPC_PROXY -u grpc_proxy -u NO_PROXY -u no_proxy \
       node scripts/drive-backing-movie.mjs
   ```
2. **The Bash sandbox.** Loopback is blocked unless allowlisted. This repo's
   `.claude/settings.json` sets `sandbox.network.allowLocalBinding: true` to
   permit it permanently (takes effect on the next session; mid-session you can
   pass `dangerouslyDisableSandbox` for a one-off, or use `/sandbox`).

The control server **only binds loopback** (`127.0.0.1`, non-loopback binds are
refused by design), so there's nothing to expose on the LAN — loopback is the
only path, and mirrored networking is what makes it reachable from WSL.

`scripts/drive-backing-movie.mjs` notes:
- Uses Node's built-in `WebSocket` — **no npm deps**.
- The `MOVIE` path must be a **Windows** path (`E:\…`) because the *Windows* app
  opens/copies it. Generate a throwaway clip with ffmpeg (on WSL or Windows):
  `ffmpeg -y -f lavfi -i "testsrc=duration=2:size=320x240:rate=15" -pix_fmt yuv420p E:\…\movie.mp4`.
- `save_bundle` returns a **backslashed** relative dir; normalise to POSIX before
  resolving it under `/mnt/e` for file checks.

## Historical note — the composer self-deadlock (fixed)

While first running this scenario, `save_bundle` hung forever (no reply, applier
thread wedged). Root cause: the Tauri frontend's `apply_request`
(`tauri-app/src-tauri/src/control.rs`) held the composer `Mutex` across
host-command dispatch, and `save_bundle` / `load_bundle` / `split_bundle` re-lock
that same composer through `AppState` — a `std::sync::Mutex` is **not** reentrant,
so it self-deadlocked. Host commands that lock other state (`query_dirty`,
`attach_video`) worked, which is why the hang only appeared at the first
composer-relocking command. Fixed by dispatching host commands **without** the
composer lock (mirroring the single-threaded TUI shell); guarded by
`control::tests::host_command_does_not_deadlock_when_it_relocks_the_composer`.
The lesson for new host commands: **never assume the composer lock is free to
take inside a host command** — the dispatch path must not hold it.

## See also

- [`AGENT-CONTROL.md`](AGENT-CONTROL.md) — the control protocol & vocabulary
- [`BACKING-MOVIE-SCENARIO.md`](BACKING-MOVIE-SCENARIO.md) — the scenario this runbook serves
- `scripts/drive-backing-movie.mjs` — the WSL-side attach-and-drive driver
