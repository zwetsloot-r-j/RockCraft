# M10-B — `SplitBundle` host command: save kept parts as standalone bundles

> Milestone: M10 — Split & Trim into Pieces · Issue: #218 · Suggested tier: sonnet
> Branch: `claude/m10-split-host-command`
> Depends on: M10-A (#217, `core::segment`)

## Goal

Add the app-level workflow that turns the loaded piece into one or more
standalone part bundles. Each **kept** segment becomes its own bundle: a subset
MIDI (notes shifted to t=0 via M10-A), a **full copy** of the backing/video
media (reference + offset, no re-encode), and a `meta.json` with the derived
offsets. **Discarded** segments are simply not listed — which doubles as
trimming.

## Context

- The agent-control surface has two tiers (see `CLAUDE.md` → "single source of
  truth"). This is I/O (disk copy, MIDI write), so it **must** be a
  `control::HostCommand` (`crates/control/src/host.rs`), **not** a `core::Action`.
- `HostCommand` variants dispatch through the exhaustive `HostServices` trait
  match in each frontend (`crates/tui`, `tauri-app/src-tauri`); the `host.rs`
  parity tests (`every_variant_round_trips`,
  `host_command_names_is_exhaustive_and_matches_variants`,
  `host_help_matches_host_command_names_exactly`,
  `documented_params_build_via_from_name`) enforce the catalog. Adding a variant
  fails to compile until both frontends handle it — that is intended.
- Bundle writing precedent: `crates/import/src/writer.rs::write_chart_bundle_full`
  writes `song.mid` + `meta.json` with optional `backing`/`video`. The save path
  the frontends already use for `SaveBundle`/`RecordSave` is the model for slug +
  destination handling (`SaveDest::Library { name }`).
- Media files live **in the source (loaded) bundle** next to `song.mid`
  (`backing.wav`, `source.<ext>`); the command copies them into each part bundle
  unchanged. `core::segment::slice_segment` supplies the derived
  `BackingTrack`/`BackgroundVideo` (same `file`, shifted offsets).

## What to do

1. **Add the command (`crates/control/src/host.rs`).**

   ```rust
   /// A kept part to write as its own bundle. Discarded parts are omitted.
   #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
   pub struct SegmentSpec {
       pub start_us: u64,
       pub end_us: u64,
       pub name: String,   // becomes the new bundle's library slug/name
   }

   // new HostCommand variant:
   /// Slice the loaded piece into the given kept parts, writing each as a new
   /// library bundle; returns the created bundle dir paths.
   SplitBundle { segments: Vec<SegmentSpec> },
   ```

   Wire `name()` → `"split_bundle"`, add it to `all_variants()` (a 1–2 element
   `segments` sample), and add a `host_help` entry describing the `segments`
   param (`ParamInfo`). Keep the parity tests green.

2. **Implement in `HostServices` for each frontend.** For the loaded piece:
   - Error if no piece is loaded (reuse the existing "nothing loaded" error path
     of `SaveBundle`/`QueryDirty`).
   - For each `SegmentSpec`: call `core::segment::slice_segment` with the live
     timeline + the loaded `meta.backing`/`meta.video`; write a new library
     bundle (slug from `name`) containing the subset MIDI, a **copied** backing
     file and/or video file (when present), and a `meta.json` whose `backing`/
     `video` carry the derived offsets, `grid`/`key` copied from the source, and
     `origin = TrackOrigin::Edited`.
   - Return the list of created bundle dir paths.
   - Leave the **source** piece and its bundle untouched (non-destructive; the
     user discards by omission, then may delete the original separately).

3. **Reuse, don't fork, the bundle writer.** Prefer
   `write_chart_bundle_full` / the existing save helper over a new writer. If the
   loaded timeline isn't already expressible as an `ExtractedChart`, add a thin
   internal helper rather than duplicating meta-writing logic — note the choice
   in the PR.

## Tests

- `control`: the four `host.rs` parity tests stay green with the new variant;
  `host_command_from_name("split_bundle", {"segments":[...]})` round-trips.
- A frontend-side (or `import`-crate helper) test, fixture-based: load a small
  bundle with a `backing` + `video`, run `SplitBundle` with two kept segments
  (one in the middle discarded by omission) → two new bundle dirs exist, each
  with `song.mid`, the copied media files, and a `meta.json` whose
  `backing.audio_start_us` / `video.offset_us` equal source + segment start, and
  `origin == Edited`. The source bundle is unchanged.
- A piece with no media slices to MIDI-only part bundles (`backing`/`video`
  stay `None`); the command still succeeds.

## Scope boundaries (do NOT)

- Do **not** re-encode or trim media (no ffmpeg here) — copy the file as-is and
  rely on the M10-A offsets.
- Do **not** add a one-off IPC/Tauri command with no protocol counterpart — the
  capability is a `HostCommand` so both frontends and the agent socket get it
  (per `CLAUDE.md`).
- Do **not** mutate or delete the source bundle.
- UI (markers/keep-discard/naming) is M10-C / M10-D; this issue is the command +
  write path only.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] `SplitBundle` exists, round-trips through `host_command_from_name`, and
      writes kept parts as standalone bundles with copied media + derived offsets
- [ ] PR opened against `main` from the branch above, `Closes #218`
