//! M0 "Echo": connect to the piano and print live note events.
//!
//! Run on Windows (where the CASIO USB-MIDI device is visible):
//!
//! ```text
//! cargo run -p rockcraft-midi --example echo
//! ```
//!
//! Optional argument: a substring of the port name to match (default "casio").
//! Press keys on the piano; each note-on/off prints. Ctrl-C to quit.
//!
//! Each line shows, for diagnosis:
//! - `ts`  : the event timestamp from midir, set AT INPUT TIME (authoritative)
//! - `+dt` : delta vs the previous event's timestamp (your real playing rhythm)
//! - `lag` : wall-clock at print MINUS event timestamp = input→display latency.
//!   If `lag` stays small, timestamps are accurate and any perceived lag is
//!   purely console rendering. If `lag` grows during fast play, the DISPLAY is
//!   falling behind — but `ts`/`+dt` remain correct.

use rockcraft_midi::{core::NoteEventKind, LiveInput};
use std::time::{Duration, Instant};

fn main() {
    let filter = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "casio".to_string());

    let input = match LiveInput::connect(&filter) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("Could not open MIDI input: {e}");
            std::process::exit(1);
        }
    };

    println!("Connected to: {}", input.port_name());
    println!("Play the piano — events below. Ctrl-C to quit.");
    println!("Columns: ts(us)  +dt(ms)  lag(ms)  kind note vel\n");

    // Wall clock with (approximately) the same origin as midir's timestamps:
    // both start when the connection opens.
    let start = Instant::now();
    let mut prev_ts: Option<u64> = None;

    loop {
        for ev in input.events() {
            let wall_us = start.elapsed().as_micros() as u64;
            let dt_ms = prev_ts.map(|p| (ev.timestamp_us.saturating_sub(p)) as f64 / 1000.0);
            let lag_ms = (wall_us.saturating_sub(ev.timestamp_us)) as f64 / 1000.0;
            prev_ts = Some(ev.timestamp_us);

            let dt_str = match dt_ms {
                Some(d) => format!("{d:7.1}"),
                None => "      -".to_string(),
            };
            let note = ev.note.name();
            match ev.kind {
                NoteEventKind::On { velocity } => println!(
                    "{:>10}  {}  {:6.1}  ON   {:<4} vel {}",
                    ev.timestamp_us,
                    dt_str,
                    lag_ms,
                    note,
                    velocity.value()
                ),
                NoteEventKind::Off => println!(
                    "{:>10}  {}  {:6.1}  OFF  {}",
                    ev.timestamp_us, dt_str, lag_ms, note
                ),
            }
        }
        // Poll cadence — rendering/printing is decoupled from MIDI timing.
        std::thread::sleep(Duration::from_millis(5));
    }
}
