// Typed wrappers for the MIDI input IPC commands and events.
//
// Source of truth on the Rust side:
//   tauri-app/src-tauri/src/midi.rs  (midi_status, mock_key commands)
//   crates/midi/src/mock.rs::KEY_MAP (key → MIDI-note mapping)

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ── Types ─────────────────────────────────────────────────────────────────────

/** Status of the active MIDI input source. */
export interface MidiStatus {
  /** `"live"` when a real USB-MIDI device is connected, `"mock"` otherwise. */
  kind: "live" | "mock";
  /** Connected port name when `kind === "live"`, `null` otherwise. */
  port: string | null;
}

/**
 * A serialised MIDI event payload from the backend `midi_event` push event.
 *
 * Mirror of `tauri-app/src-tauri/src/midi.rs::NoteEventPayload`.
 * The backend projects `NoteEvent` into this flat, JSON-friendly shape so
 * the frontend never has to handle serde enum tags.
 */
export interface NoteEventPayload {
  /** MIDI note number (0–127). */
  note: number;
  /** `true` = note-on, `false` = note-off. */
  on: boolean;
  /** Velocity (0–127); 0 for note-offs. */
  velocity: number;
  /** Engine timestamp in microseconds (preserved from the source). */
  timestamp_us: number;
}

// ── Key map ───────────────────────────────────────────────────────────────────

/**
 * Computer-key → MIDI-note mapping for the mock keyboard.
 *
 * SOURCE OF TRUTH: `crates/midi/src/mock.rs::KEY_MAP`.
 * The number row plays a C-major (white-key) scale from middle C (60):
 *
 * ```
 *   1   2   3   4   5   6   7   8   9   0
 *   C4  D4  E4  F4  G4  A4  B4  C5  D5  E5
 *   60  62  64  65  67  69  71  72  74  76
 * ```
 *
 * Keep this in sync with the Rust `KEY_MAP` constant when that changes.
 */
export const MOCK_KEY_MAP: ReadonlyArray<[string, number]> = [
  ["1", 60], // C4
  ["2", 62], // D4
  ["3", 64], // E4
  ["4", 65], // F4
  ["5", 67], // G4
  ["6", 69], // A4
  ["7", 71], // B4
  ["8", 72], // C5
  ["9", 74], // D5
  ["0", 76], // E5
];

/** The set of keys that map to MIDI notes (for quick membership checks). */
export const MOCK_KEY_SET: ReadonlySet<string> = new Set(
  MOCK_KEY_MAP.map(([k]) => k),
);

// ── IPC helpers ───────────────────────────────────────────────────────────────

/** Event name the backend emits serialised `NoteEvent`s on. */
const EVENT_MIDI = "midi_event";

/**
 * Subscribe to `midi_event` backend events.
 *
 * Returns the Tauri unlisten function; call it in `onCleanup`.
 *
 * **Debugging snippet** (paste in the webview console):
 * ```js
 * const { listen } = window.__TAURI__.event;
 * listen('midi_event', e => console.log('midi', e.payload));
 * ```
 */
export function onMidiEvent(
  cb: (ev: NoteEventPayload) => void,
): Promise<UnlistenFn> {
  return listen<NoteEventPayload>(EVENT_MIDI, (e) => cb(e.payload));
}

/** Fetch the current MIDI input status (kind + port name). */
export function midiStatus(): Promise<MidiStatus> {
  return invoke<MidiStatus>("midi_status");
}

/**
 * Simulate a computer-key press or release on the mock keyboard.
 *
 * `down=true` triggers a note-on; the backend auto-enqueues a note-off
 * after the sustain window, so `down=false` is accepted but is a no-op.
 *
 * Rejects when a live device is active (mock keys must not inject fake events).
 *
 * Only call this when `midiStatus().kind === "mock"`.
 */
export function mockKey(key: string, down: boolean): Promise<void> {
  // The Rust side takes a `char`; single-character strings map cleanly.
  const ch = key.length === 1 ? key : key[0];
  return invoke<void>("mock_key", { key: ch, down });
}
