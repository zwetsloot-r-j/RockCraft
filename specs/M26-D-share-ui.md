# M26-D — Network panel in the desktop library

> Milestone: M26 — LAN song sharing · Issue: #348 · Suggested tier: sonnet
> Branch: `claude/m26-share-ui` · Depends on: M26-B (#346), M26-C (#347)

## Goal

Share, browse, download and send songs from the desktop app's library
screen. No agent socket is needed.

## Context

- Host commands from M26-B/C (Tauri IPC pairs):
  - `share_start` / `share_stop` / `share_status`;
  - `share_discover`;
  - `share_list`;
  - `share_download` / `share_upload` / `share_transfers`.
- M26-A's library `x` export / `i` import keys and the Replace / Keep both /
  Cancel prompt for `exists:<slug>`. Reuse that prompt here.
- `tauri-app/src/screens/library/LibraryScreen.tsx` (list, keys, rescans via
  `scanLibrary`).

## What to do

### 1. Sharing toggle

- Put a "Share my library on the network" toggle at the top of the Network
  panel. On: `share_start`, and show "Sharing as `<name>` at
  `192.168.1.5:47800`". Off: `share_stop`.
- Remember the choice in localStorage (`rockcraft.share.enabled`, in
  try/catch). When the app starts with it on, call `share_start` from the app
  shell, not from the library screen, so sharing runs whatever screen is
  open.
- If `share_status.discoverable` is false, show a quiet hint: "Other devices
  may need your address to connect".

### 2. Network panel

`N` on the library screen toggles a split view: local songs on the left, the
Network panel on the right. Keys below are for the panel; Tab switches focus
between the two lists.

- **Servers list:** discovered peers (`share_discover` on open, and again on
  `r`) plus saved manual addresses (`+` opens an `host:port` input; stored
  in localStorage `rockcraft.share.servers`; `Delete` removes one). On
  selecting a server, `share_list` it. Show "can't reach `<addr>`" on error.
- **Remote songs list** for the selected server: name, duration, note count,
  🎬 if it has video, size in MB, and a badge from `local`:
  - "in library" (`same`)
  - "different version" (`different`)
  - nothing (`absent`)

  `Enter` / `d` downloads it. On `exists:` show the Replace / Keep both /
  Cancel prompt. Replace re-runs with `overwrite: true`. Keep both downloads
  into a free `<slug>-N`; add `rename_to: Option<String>` to `share_download`
  for this, and update the M26-B catalog entry.
- **Send:** with focus in the local list and a server selected, `u` uploads
  the selected song (`share_upload`), with the same conflict prompt.
- **Transfers:** a strip under the lists polls `share_transfers` every 500 ms
  while any transfer is running:
  - per transfer: name, ↓/↑, a progress bar, MB done / total;
  - a finished download triggers a rescan of the local list and flashes
    "downloaded `<name>`";
  - failures show their detail.

### 3. Polish

- Add `N`, `x`, `i`, `u` and Tab to the library key legend.
- Mouse: click selects in any list, double-click a remote song downloads it.
- Every async call shows errors in the flash line. Nothing blocks the UI
  thread.

## Tests

- Pure helpers in their own `.ts` modules with vitest:
  - the transfer progress formatting (MB, percent);
  - the `local` badge mapping;
  - the manual address validation (`host:port`, port 1–65535; IPv6
    `[::1]:47800` accepted);
  - choosing a free `-N` name for Keep both;
  - the persisted settings load/save, including localStorage throwing.
- `tsc`, vitest and the Vite build stay green.

## Scope boundaries (do NOT)

- No TUI UI (the host commands already cover the TUI).
- No changes to the HTTP API beyond `rename_to`. No auth.

## Acceptance

- [ ] Frontend `tsc`/vitest/build green; `cargo fmt/clippy/test` green; Tauri crate tests green locally
- [ ] Manual, on two machines:
  - [ ] toggle sharing on and see the peer appear
  - [ ] download a video song with live progress
  - [ ] send a song back
  - [ ] resolve a conflict with Keep both
- [ ] PR against `main` from the branch above, `Closes #348`
