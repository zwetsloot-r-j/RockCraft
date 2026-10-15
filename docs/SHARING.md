# Sharing songs on the home network

Every RockCraft can share its library with other RockCrafts on the same home
network. There's no central server: each instance can serve its own songs
and download from (or upload to) any other. Songs travel whole, video
included, as `.rcsong` archives (the same files the library's `x`/`i` export
and import use, see M26-A).

## Starting and stopping

- **Agent socket:** `share_start` (optionally `{ "port": 47900 }`),
  `share_stop`, `share_status`. `share_start` returns the addresses other
  devices connect to, e.g. `"192.168.1.5:27800"`.
- **Desktop app:** the library's Network panel (M26-D).

The server listens on port **27800**, or the next free port up to 27809.
When all ten are taken it takes any free port the OS offers; discovery
advertises the real port, and `share_status` shows it for manual entry.
(The default sits below the OS ephemeral port ranges: WSL2 in mirrored
networking mode reserves Linux's ephemeral range — e.g. 44620–48715 — on the
Windows side, so a default inside it could never bind there.)

**Windows Firewall** asks for permission the first time sharing starts. Allow
it on **private networks** (your home network). Without that, other devices
can't connect.

## Finding each other

While sharing, an instance advertises itself on the network over mDNS (service
`_rockcraft._tcp`, the mechanism printers use). `share_discover` (or the
desktop Network panel) lists the sharing instances it finds within 1.5 s,
excluding itself. Typing `host:port` always works as a fallback.

When peers don't appear:

- Both machines must be on the **same network**: the same Wi-Fi, and not a
  guest network or a separate VLAN.
- Some routers have **"AP isolation" / "client isolation"**, which blocks
  devices from seeing each other. Turn it off, or use a wired connection.
- **Firewall:** allow RockCraft on private networks (mDNS uses UDP port 5353).
- `share_status` reports `discoverable: false` plus a `discovery_error` when
  this machine couldn't advertise at all. Others can still connect by
  address.

## Using another instance

| Host command | What it does |
|---|---|
| `share_list { server }` | The songs `server` (`"host:port"`) shares, each with `local`: `same` (you have this chart), `different` (you have a song with this name but a different chart), `absent` |
| `share_download { server, id, overwrite, rename_to? }` | Download in the background; returns `{ transfer_id }` |
| `share_upload { server, bundle, overwrite, rename_to? }` | Upload a bundle folder in the background; returns `{ transfer_id }` |
| `share_transfers` | Progress and outcome of every transfer (`running` / `done` / `failed` / `exists`) |

A name clash never overwrites silently. Without `overwrite` the transfer ends
in state `exists` with the clashing `slug`; retry with `overwrite: true`
(replace) or `rename_to: "<slug>-2"` (keep both).

## HTTP API (v1)

Plain HTTP, JSON, under `/api/v1`:

| Method & path | Response |
|---|---|
| `GET /info` | `{ app: "rockcraft", api: 1, name, songs }` |
| `GET /songs` | `[{ id, name, origin, note_count, duration_us, has_backing, has_video, size_bytes, chart_hash }]` |
| `GET /songs/{id}/archive` | The `.rcsong` (`application/zip`) |
| `PUT /songs/{slug}?overwrite=0\|1` | Upload a `.rcsong` → `201`; `409` name taken; `400` bad archive or name; `413` over 4 GiB |

A song `id` is `<root>~<folder>`, e.g. `library~frog-song` or
`import-out~shimanouta-1786440327`. Everything the library browser lists is
shared, and uploads land in `library/`.

## Security: open on the home network

By decision there's **no PIN, login or encryption**: anyone on your home
network can list, download and upload while sharing is on. Two safeguards:

- The server refuses every request that doesn't come from a home-network
  address (loopback, `10.x`, `172.16–31.x`, `192.168.x`, link-local, IPv6
  ULA/link-local) with `403`. Never forward the port on your router.
- Uploads are unpacked strictly: plain file names only, only the files the
  song's `meta.json` accounts for, a 4 GiB cap, and atomic placement. A bad
  upload can't write outside its own song folder or leave half a song behind.

Turn sharing off on networks you don't trust (cafés, hotels).
