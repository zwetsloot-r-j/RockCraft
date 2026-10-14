# M26-C — Find sharing peers automatically (mDNS)

> Milestone: M26 — LAN song sharing · Issue: #347 · Suggested tier: sonnet
> Branch: `claude/m26-share-discovery` · Depends on: M26-B (#346)

## Goal

Sharing instances appear in a list without anyone typing an IP address.
Advertise and browse with mDNS/DNS-SD, the same mechanism printers and
Chromecasts use. Manual `host:port` entry from M26-B stays as the fallback.

## Context

- M26-B: `ShareServer` (port, host name), `ShareService`, host commands
  `share_start` / `share_stop` / `share_status`, and the client's `info()`.

## What to do

- Add `mdns-sd` (pure Rust, no system daemon) to `crates/share`. New module
  `discovery.rs`.
- **Advertise** while the server runs: service type `_rockcraft._tcp.local.`,
  instance name = host name, the server's port, and TXT `api=1`, `name=<host
  name>`. Unregister on `stop`, and on drop.
- **Browse** for `timeout` ms (default 1500) and collect resolved services:

  ```rust
  pub struct Peer { pub name: String, pub addr: String /* "ip:port" */, pub api: u32 }
  pub fn discover(timeout: Duration, exclude_port_on_self: Option<u16>) -> Vec<Peer>;
  ```

  - Prefer an IPv4 address; skip loopback.
  - Skip **this** instance (same port and one of our own addresses).
  - Remove duplicates by `addr`, and sort by name.
- **Host command** `share_discover { timeout_ms: Option<u32> }` → `[Peer]`, in
  both frontends, with parity and catalog entries and a Tauri IPC pair.
- **Robustness:** if the mDNS daemon can't start (no multicast interface,
  port 5353 blocked), `share_start` still succeeds, and `share_status` gains
  `discoverable: false` plus a `discovery_error`. `share_discover` returns
  `[]` plus no error; the UI shows the manual entry.
- `docs/SHARING.md`: discovery, and what to check when peers don't appear
  (same Wi-Fi network/VLAN, router "AP/client isolation", firewall).

## Tests

- Advertise and browse **in the same process** on loopback/multicast where
  available: the browser finds the advertised instance, with name, port and
  `api` from the TXT. Make it skip, not fail, when the CI runner has no
  multicast (detect by the daemon start error) and say so in the test name
  or a comment.
- Pure unit tests for the peer filtering (self-exclusion, IPv4 preference,
  de-duplication, sorting) over hand-built resolved records.
- Host-command parity.

## Scope boundaries (do NOT)

- No UI (M26-D). No changes to the HTTP API.

## Acceptance

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace` green
- [ ] Manual: two machines on the same Wi-Fi see each other via `share_discover`
- [ ] PR against `main` from the branch above, `Closes #347`
