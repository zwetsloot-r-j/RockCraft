# M26-B — LAN share server + client + host commands

> Milestone: M26 — LAN song sharing · Issue: #346 · Suggested tier: opus
> Branch: `claude/m26-share-server` · Depends on: M26-A (#345)

## Goal

Every RockCraft can share its library on the home network (peer-to-peer:
there's no central server, and any instance serves and downloads). Another
instance lists the songs, downloads them (always **with** video), and uploads
its own. Decided with the user:

- peer-to-peer;
- downloads include the video;
- **no PIN or auth:** open to the home network.

## Context

- M26-A: `rockcraft-share::archive` (`pack`, `unpack`, `chart_hash`,
  `bundle_files`), the `.rcsong` format, and the `exists:<slug>` convention.
- Library listing: `rockcraft_midi::bundle::{default_scan_roots, list_library,
  library_root, slug}`.
- `tokio` is already a workspace dependency (control socket, `crates/control`).
- Host commands: `crates/control/src/host.rs`, plus the exhaustive `match` in
  `tauri-app/src-tauri/src/control.rs` and `crates/tui/src/app.rs`.
  Precedent for a long background job polled for status: the import pipeline
  (`import_start` + its status query).

## What to do

### 1. Server (`crates/share/src/server.rs`)

axum (add to the workspace) on its **own** tokio runtime thread, so it runs
whichever screen is open.

```rust
pub struct ShareServer { /* runtime thread, shutdown handle, bound addr */ }
impl ShareServer {
    /// Bind 0.0.0.0:<port>; port 47800 by default, trying 47801..=47809 when
    /// busy. `roots` = scan roots for listing, `library` = where uploads land.
    pub fn start(port: Option<u16>, roots: Vec<PathBuf>, library: PathBuf, name: String)
        -> Result<ShareServer, ShareError>;
    pub fn port(&self) -> u16;
    pub fn stop(self);
}
```

All JSON, under `/api/v1`:

| Method & path | Response |
|---|---|
| `GET /info` | `{ "app": "rockcraft", "api": 1, "name": <host name>, "songs": <count> }` |
| `GET /songs` | `[{ id, name, origin, note_count, duration_us, has_backing, has_video, size_bytes, chart_hash }]`, sorted by name |
| `GET /songs/{id}/archive` | the `.rcsong`, `application/zip`, `Content-Length`, `Content-Disposition: attachment; filename="<name>.rcsong"`. Pack to a temp file, then stream it; never buffer the whole file in memory |
| `PUT /songs/{slug}?overwrite=0\|1` | body = `.rcsong`, streamed to a temp file then `unpack`ed into `library/<slug>` → `201 { "id" }`; `409 { "error": "exists" }`; `400` bad archive / bad slug (`slug(x) != x`); `413` over 4 GiB |

- **Song id** = `<root label>~<folder name>`, where the root label is
  `library` / `recordings` / `import-out`. It's percent-encoded in paths. An
  unknown id gives `404`.
- **Home network only:** reject (`403`) any peer whose address isn't
  loopback, RFC 1918, link-local, or IPv6 ULA/link-local. No other auth, by
  decision.
- **Host name:** `COMPUTERNAME` / `HOSTNAME` env, else `"RockCraft"`.
- Log one line per request to stderr (method, path, peer, status).

### 2. Client (`crates/share/src/client.rs`)

Blocking, on `ureq` with default features off (plain HTTP only).

```rust
pub fn info(server: &str) -> Result<ServerInfo, ShareError>;          // "host:port"
pub fn list(server: &str) -> Result<Vec<RemoteSong>, ShareError>;
/// Download into library/<slug(name)>; streams to a temp file, then unpack.
pub fn download(server: &str, song: &RemoteSong, library: &Path, overwrite: bool,
                progress: &dyn Fn(u64, u64)) -> Result<PathBuf, ShareError>;
pub fn upload(server: &str, bundle: &Path, overwrite: bool,
              progress: &dyn Fn(u64, u64)) -> Result<(), ShareError>;
```

`409` maps to `ShareError::Exists(slug)`, and its `Display` is
`exists:<slug>`, the same convention as M26-A.

### 3. `ShareService` (`crates/share/src/service.rs`)

One struct that both frontends hold, so each `match` arm is a one-liner:

- the optional running `ShareServer`;
- a transfer table (`Arc<Mutex<Vec<Transfer>>>`), where each transfer runs on
  its own thread: `Transfer { id, direction: down|up, server, name,
  bytes_done, bytes_total, state: running|done|failed{detail}|exists{slug} }`.

### 4. Host commands

| Command | Params | Returns |
|---|---|---|
| `share_start` | `port: Option<u16>` | `{ port, addresses: ["192.168.1.5:47800", …] }` (non-loopback IPv4 of this machine) |
| `share_stop` | — | `null` |
| `share_status` | — | `{ running, port?, addresses, name }` |
| `share_list` | `server` | remote songs + `local`: `"same"` (a local song with that name and the same `chart_hash`), `"different"` (same name, other hash), `"absent"` |
| `share_download` | `server, id, overwrite` | `{ transfer_id }` (starts a background transfer) |
| `share_upload` | `server, bundle, overwrite` | `{ transfer_id }` |
| `share_transfers` | — | the transfer table (finished entries stay until the next `share_start`/restart) |

Both frontends implement them, with names-list, catalog and parity entries.
The Tauri IPC commands are paired 1:1 (`share_*`).

### 5. Docs

- `docs/SHARING.md`: how it works, the endpoint table, the open-on-LAN
  decision and its risk, and the Windows Firewall prompt on first start
  (allow "Private networks").
- Update the `docs/AGENT-CONTROL.md` host command table.

## Tests

- **Server** (in-process, ephemeral port), against a temp library holding two
  bundles:
  - `/info`;
  - `/songs` fields and `chart_hash`;
  - archive download, then `unpack`, is byte-identical;
  - unknown id → 404;
  - upload → 201, again → 409, `overwrite=1` → 201;
  - bad slug → 400;
  - a garbage body → 400 and no folder left behind.
- **Private-address filter:** a unit test over a table of IPs (`192.168.1.2`,
  `10.0.0.1`, `172.16.5.4`, `169.254.1.1`, `127.0.0.1`, `fd00::1`,
  `fe80::1` allowed; `8.8.8.8`, `172.32.0.1`, `2001:db8::1` rejected).
- **Client against the server:** `list`, `download` (progress ends at
  total), `upload`, and `Exists` on a conflict.
- **`ShareService`:** a download transfer goes running → done, and a
  conflict ends in `exists{slug}`.
- **`share_list` `local` classification:** same / different / absent.
- Host-command parity tests.

## Scope boundaries (do NOT)

- No discovery (M26-C) and no UI (M26-D). In this spec the server is started
  and used by host command only.
- No auth, TLS, or internet exposure. No resume of interrupted transfers
  (a retry restarts it).
- Don't change M26-A's archive rules.

## Acceptance

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace` green
- [ ] Tauri crate tests green locally (Windows cargo)
- [ ] Manual: two machines (or two instances on two ports), a list plus a download of a video song
- [ ] PR against `main` from the branch above, `Closes #346`
