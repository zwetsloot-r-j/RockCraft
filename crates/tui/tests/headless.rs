//! Headless TUI integration tests.
//!
//! These drive Record/Play screens with a `ScriptedSource` and ratatui's
//! `TestBackend`, asserting on scoring/highway state and render output from a
//! known input — no terminal or piano required.

use ratatui::{backend::TestBackend, Terminal};
use rockcraft_core::{
    score, ExpectedNote, MidiNote, NoteEvent, NoteEventKind, ScoreConfig, ScoreReport, Timing,
    Velocity,
};
use rockcraft_midi::{NoteSource, ScriptedSource};

// ── helpers ──────────────────────────────────────────────────────────────────

fn note(n: u8) -> MidiNote {
    MidiNote::new(n).unwrap()
}
fn vel(v: u8) -> Velocity {
    Velocity::new(v).unwrap()
}
fn on(n: u8, ts: u64) -> NoteEvent {
    NoteEvent::on(note(n), vel(80), ts)
}
fn off(n: u8, ts: u64) -> NoteEvent {
    NoteEvent::off(note(n), ts)
}
fn expect_note(n: u8, ts: u64) -> ExpectedNote {
    ExpectedNote::new(note(n), ts)
}

/// Drain all events from a `ScriptedSource` after advancing its clock to `end_us`.
fn drain_all(src: &mut ScriptedSource, end_us: u64) -> Vec<NoteEvent> {
    src.advance(end_us);
    src.events()
}

// ── ScriptedSource unit tests ─────────────────────────────────────────────────

#[test]
fn scripted_source_port_name() {
    let src = ScriptedSource::new(vec![]);
    assert_eq!(src.port_name(), "scripted");
}

#[test]
fn scripted_source_events_emit_only_after_advance() {
    let mut src = ScriptedSource::new(vec![on(60, 1_000), on(62, 2_000)]);
    // Nothing before advance.
    assert!(src.events().is_empty());
    src.advance(1_000);
    let batch = src.events();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].note.value(), 60);
    // Second drain should be empty.
    assert!(src.events().is_empty());
}

#[test]
fn scripted_source_queue_empties_exactly_once() {
    let mut src = ScriptedSource::new(vec![on(60, 100)]);
    src.advance(200);
    assert_eq!(src.events().len(), 1);
    src.advance(1_000_000); // way past the end
    assert!(src.events().is_empty());
}

// ── Play / scoring tests ──────────────────────────────────────────────────────

/// A perfectly on-time scripted timeline should produce all Perfect hits and
/// zero misses/extras.
#[test]
fn scoring_on_time_notes_all_perfect() {
    let expected = vec![
        expect_note(60, 0),
        expect_note(64, 500_000),
        expect_note(67, 1_000_000),
    ];
    let played_events = vec![on(60, 0), on(64, 500_000), on(67, 1_000_000)];

    let mut src = ScriptedSource::new(played_events.clone());
    let drained = drain_all(&mut src, 2_000_000);

    let report: ScoreReport = score(&expected, &drained, ScoreConfig::default());

    assert_eq!(report.hits, 3);
    assert_eq!(report.misses, 0);
    assert_eq!(report.extras, 0);
    assert!(report.judgments.iter().all(|j| matches!(
        j,
        rockcraft_core::NoteJudgment::Hit {
            timing: Timing::Perfect,
            ..
        }
    )));
}

/// An off-time timeline where every note arrives beyond the good window should
/// produce all misses.
#[test]
fn scoring_off_time_notes_all_miss() {
    let expected = vec![expect_note(60, 1_000_000)];
    // Played 300 ms late — beyond the 150 ms good window.
    let played = vec![on(60, 1_300_000)];

    let mut src = ScriptedSource::new(played);
    let drained = drain_all(&mut src, 2_000_000);

    let report = score(&expected, &drained, ScoreConfig::default());

    assert_eq!(report.hits, 0);
    assert_eq!(report.misses, 1);
    assert_eq!(report.extras, 1); // unmatched played note
}

/// A completely missing note (nothing played) should count as a miss with no extras.
#[test]
fn scoring_missing_note_is_miss() {
    let expected = vec![expect_note(60, 500_000)];
    let mut src = ScriptedSource::new(vec![]); // nothing played
    let drained = drain_all(&mut src, 1_000_000);

    let report = score(&expected, &drained, ScoreConfig::default());

    assert_eq!(report.hits, 0);
    assert_eq!(report.misses, 1);
    assert_eq!(report.extras, 0);
}

/// Early hit within the good window should be classified `Early`.
#[test]
fn scoring_early_hit_within_good_window() {
    let expected = vec![expect_note(60, 1_000_000)];
    // 100 ms early — within the 150 ms good window, outside 50 ms perfect.
    let played = vec![on(60, 900_000)];

    let mut src = ScriptedSource::new(played);
    let drained = drain_all(&mut src, 2_000_000);

    let report = score(&expected, &drained, ScoreConfig::default());

    assert_eq!(report.hits, 1);
    assert!(matches!(
        report.judgments[0],
        rockcraft_core::NoteJudgment::Hit {
            timing: Timing::Early,
            error_us: -100_000
        }
    ));
}

// ── Record screen tests ───────────────────────────────────────────────────────

// These tests live in the integration test crate but access `RecordScreen`
// directly through the public API (no TUI loop needed).

mod record_screen {
    use rockcraft_tui::record::RecordScreen;

    use super::*;

    #[test]
    fn record_screen_captures_events_in_order() {
        let mut rec = RecordScreen::new();
        rec.ingest(on(60, 0));
        rec.ingest(on(64, 1_000));
        rec.ingest(off(60, 2_000));

        let evs = rec.recorded_events();
        assert_eq!(evs.len(), 3);
        assert_eq!(evs[0].timestamp_us, 0);
        assert_eq!(evs[1].timestamp_us, 1_000);
        assert_eq!(evs[2].timestamp_us, 2_000);
    }

    #[test]
    fn record_screen_preserves_note_on_off_kinds() {
        let mut rec = RecordScreen::new();
        rec.ingest(on(60, 0));
        rec.ingest(off(60, 500));

        let evs = rec.recorded_events();
        assert!(matches!(evs[0].kind, NoteEventKind::On { .. }));
        assert_eq!(evs[1].kind, NoteEventKind::Off);
    }

    #[test]
    fn record_screen_rebases_first_event_to_zero() {
        // The origin rebase means the first event's timestamp becomes 0 in
        // the buffer, regardless of the raw timestamp value.
        let mut rec = RecordScreen::new();
        rec.ingest(on(60, 50_000)); // first event at t=50 ms
        rec.ingest(on(64, 60_000)); // second event 10 ms later

        let evs = rec.recorded_events();
        assert_eq!(evs[0].timestamp_us, 0, "first event rebased to 0");
        assert_eq!(evs[1].timestamp_us, 10_000, "delta preserved");
    }

    #[test]
    fn record_screen_via_scripted_source_lands_in_order() {
        // Feed events from a ScriptedSource into RecordScreen and verify the
        // buffer contents match what was sent.
        let raw = vec![on(60, 0), on(62, 1_000), off(60, 2_000), off(62, 3_000)];
        let mut src = ScriptedSource::new(raw.clone());
        src.advance(5_000);
        let batch = src.events();

        let mut rec = RecordScreen::new();
        for ev in batch {
            rec.ingest(ev);
        }

        let evs = rec.recorded_events();
        assert_eq!(evs.len(), 4);
        // After rebase the first is 0, rest preserve their deltas.
        assert_eq!(evs[0].timestamp_us, 0);
        assert_eq!(evs[1].timestamp_us, 1_000);
        assert_eq!(evs[2].timestamp_us, 2_000);
        assert_eq!(evs[3].timestamp_us, 3_000);
    }
}

// ── TestBackend smoke renders ─────────────────────────────────────────────────

mod smoke_render {
    use rockcraft_tui::app::Shell;

    use super::*;

    fn make_terminal(w: u16, h: u16) -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(w, h)).expect("TestBackend creation failed")
    }

    #[test]
    fn menu_renders_non_empty_non_panicking() {
        let src: Box<dyn NoteSource> = Box::new(ScriptedSource::new(vec![]));
        let shell = Shell::new(src, None, None);
        let mut terminal = make_terminal(80, 24);

        terminal.draw(|f| shell.render(f)).expect("draw panicked");

        let buf = terminal.backend().buffer();
        // At least one non-space cell means something rendered.
        let non_empty = buf.content().iter().any(|cell| cell.symbol() != " ");
        assert!(non_empty, "Menu screen rendered an entirely blank buffer");
    }

    #[test]
    fn record_screen_render_non_empty() {
        use rockcraft_tui::record::RecordScreen;

        let mut rec = RecordScreen::new();
        rec.ingest(on(60, 0));
        rec.ingest(on(64, 1_000));

        let mut terminal = make_terminal(80, 24);
        terminal
            .draw(|f| rec.draw(f, f.area()))
            .expect("draw panicked");

        let buf = terminal.backend().buffer();
        let non_empty = buf.content().iter().any(|cell| cell.symbol() != " ");
        assert!(non_empty, "Record screen rendered an entirely blank buffer");
    }
}
