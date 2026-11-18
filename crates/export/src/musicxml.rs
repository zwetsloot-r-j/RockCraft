//! MusicXML 4.0 writer for the notation model.
//!
//! One grand-staff piano part: staff 1 (treble, right hand) in voice 1, then a
//! `<backup>` of the whole bar, then staff 2 (bass, left hand) in voice 5. The
//! output is small and regular, so it is written by hand with escaping rather
//! than through an XML library.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rockcraft_core::notation::{
    Accidental, BaseValue, Event, EventKind, NoteValue, Score, WrittenNote, DIVISIONS,
};

/// A complete MusicXML 4.0 `score-partwise` document for one grand-staff piano
/// part. Pure: returns the text, never touches the disk.
pub fn to_musicxml(score: &Score, title: &str) -> String {
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    x.push_str(
        "<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" \
         \"http://www.musicxml.org/dtds/partwise.dtd\">\n",
    );
    x.push_str("<score-partwise version=\"4.0\">\n");
    let _ = writeln!(
        x,
        "  <work><work-title>{}</work-title></work>",
        escape(title)
    );
    x.push_str(
        "  <identification><encoding><software>RockCraft</software></encoding></identification>\n",
    );
    x.push_str("  <part-list>\n");
    x.push_str("    <score-part id=\"P1\"><part-name>Piano</part-name></score-part>\n");
    x.push_str("  </part-list>\n");
    x.push_str("  <part id=\"P1\">\n");

    for (i, measure) in score.measures.iter().enumerate() {
        let _ = writeln!(x, "    <measure number=\"{}\">", measure.number);
        if i == 0 {
            attributes(&mut x, score);
        }
        if let Some(bpm) = measure.tempo {
            tempo(&mut x, bpm);
        }
        for event in &measure.staves[0] {
            write_event(&mut x, event, score.bar_ticks, 1, 1);
        }
        let _ = writeln!(
            x,
            "      <backup><duration>{}</duration></backup>",
            score.bar_ticks
        );
        for event in &measure.staves[1] {
            write_event(&mut x, event, score.bar_ticks, 5, 2);
        }
        x.push_str("    </measure>\n");
    }

    x.push_str("  </part>\n");
    x.push_str("</score-partwise>\n");
    x
}

/// `path` with `.musicxml` appended when it has no extension.
pub fn with_musicxml_extension(path: &Path) -> PathBuf {
    if path.extension().is_some() {
        path.to_path_buf()
    } else {
        let mut name = path.as_os_str().to_os_string();
        name.push(".musicxml");
        PathBuf::from(name)
    }
}

fn attributes(x: &mut String, score: &Score) {
    x.push_str("      <attributes>\n");
    let _ = writeln!(x, "        <divisions>{DIVISIONS}</divisions>");
    let _ = writeln!(
        x,
        "        <key><fifths>{}</fifths><mode>{}</mode></key>",
        score.key_fifths,
        if score.minor { "minor" } else { "major" }
    );
    let _ = writeln!(
        x,
        "        <time><beats>{}</beats><beat-type>{}</beat-type></time>",
        score.time_sig.beats_per_bar, score.time_sig.beat_unit
    );
    x.push_str("        <staves>2</staves>\n");
    x.push_str("        <clef number=\"1\"><sign>G</sign><line>2</line></clef>\n");
    x.push_str("        <clef number=\"2\"><sign>F</sign><line>4</line></clef>\n");
    x.push_str("      </attributes>\n");
}

fn tempo(x: &mut String, bpm: u32) {
    x.push_str("      <direction placement=\"above\">\n");
    let _ = writeln!(
        x,
        "        <direction-type><metronome><beat-unit>quarter</beat-unit>\
         <per-minute>{bpm}</per-minute></metronome></direction-type>"
    );
    x.push_str("        <staff>1</staff>\n");
    let _ = writeln!(x, "        <sound tempo=\"{bpm}\"/>");
    x.push_str("      </direction>\n");
}

fn write_event(x: &mut String, event: &Event, bar_ticks: u32, voice: u8, staff: u8) {
    match &event.kind {
        EventKind::BarRest => {
            x.push_str("      <note>\n");
            x.push_str("        <rest measure=\"yes\"/>\n");
            let _ = writeln!(x, "        <duration>{bar_ticks}</duration>");
            let _ = writeln!(x, "        <voice>{voice}</voice>");
            let _ = writeln!(x, "        <staff>{staff}</staff>");
            x.push_str("      </note>\n");
        }
        EventKind::Rest => {
            x.push_str("      <note>\n");
            x.push_str("        <rest/>\n");
            let _ = writeln!(x, "        <duration>{}</duration>", event.value.ticks());
            let _ = writeln!(x, "        <voice>{voice}</voice>");
            value(x, event.value);
            let _ = writeln!(x, "        <staff>{staff}</staff>");
            x.push_str("      </note>\n");
        }
        EventKind::Chord(notes) => {
            for (i, note) in notes.iter().enumerate() {
                write_note(x, note, event.value, i > 0, voice, staff);
            }
        }
    }
}

/// One `<note>`, children in the order the MusicXML schema requires.
fn write_note(
    x: &mut String,
    note: &WrittenNote,
    note_value: NoteValue,
    chord: bool,
    voice: u8,
    staff: u8,
) {
    x.push_str("      <note>\n");
    if chord {
        x.push_str("        <chord/>\n");
    }
    let _ = write!(x, "        <pitch><step>{}</step>", note.step.letter());
    if note.alter != 0 {
        let _ = write!(x, "<alter>{}</alter>", note.alter);
    }
    let _ = writeln!(x, "<octave>{}</octave></pitch>", note.octave);
    let _ = writeln!(x, "        <duration>{}</duration>", note_value.ticks());
    if note.tie_stop {
        x.push_str("        <tie type=\"stop\"/>\n");
    }
    if note.tie_start {
        x.push_str("        <tie type=\"start\"/>\n");
    }
    let _ = writeln!(x, "        <voice>{voice}</voice>");
    value(x, note_value);
    if let Some(accidental) = note.accidental {
        let name = match accidental {
            Accidental::Sharp => "sharp",
            Accidental::Flat => "flat",
            Accidental::Natural => "natural",
        };
        let _ = writeln!(x, "        <accidental>{name}</accidental>");
    }
    let _ = writeln!(x, "        <staff>{staff}</staff>");
    if note.tie_stop || note.tie_start || note.finger.is_some() {
        x.push_str("        <notations>\n");
        if note.tie_stop {
            x.push_str("          <tied type=\"stop\"/>\n");
        }
        if note.tie_start {
            x.push_str("          <tied type=\"start\"/>\n");
        }
        // Finger number (M27-F): the model sets it on a note's first written
        // piece only, so a tied continuation never repeats it.
        if let Some(finger) = note.finger {
            let _ = writeln!(
                x,
                "          <technical><fingering>{}</fingering></technical>",
                finger.get()
            );
        }
        x.push_str("        </notations>\n");
    }
    x.push_str("      </note>\n");
}

/// `<type>` and, for a dotted value, `<dot/>`.
fn value(x: &mut String, v: NoteValue) {
    let name = match v.base {
        BaseValue::Whole => "whole",
        BaseValue::Half => "half",
        BaseValue::Quarter => "quarter",
        BaseValue::Eighth => "eighth",
        BaseValue::Sixteenth => "16th",
        BaseValue::ThirtySecond => "32nd",
    };
    let _ = writeln!(x, "        <type>{name}</type>");
    if v.dotted {
        x.push_str("        <dot/>\n");
    }
}

/// Escape the five XML special characters.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::notation::{build_score, NotationOptions};
    use rockcraft_core::{Finger, Grid, Key, MidiNote, Note, Scale, Velocity, DEFAULT_SPLIT};

    const Q: u64 = 500_000; // a quarter note at 120 BPM

    fn note(pitch: u8, start_us: u64, dur_us: u64) -> Note {
        Note {
            pitch: MidiNote::new(pitch).unwrap(),
            start_us,
            dur_us,
            velocity: Velocity::new(80).unwrap(),
            hand: None,
            finger: None,
        }
    }

    fn xml_in(notes: &[Note], bars: &[u64], key: Option<Key>, title: &str) -> String {
        let score = build_score(
            notes,
            &Grid::default_120(),
            bars,
            key,
            DEFAULT_SPLIT,
            NotationOptions::default(),
        );
        to_musicxml(&score, title)
    }

    fn xml(notes: &[Note]) -> String {
        xml_in(notes, &[], None, "Test")
    }

    /// The `<note>…</note>` blocks of a document, in order.
    fn note_blocks(doc: &str) -> Vec<&str> {
        doc.split("<note>")
            .skip(1)
            .map(|rest| rest.split("</note>").next().unwrap())
            .collect()
    }

    /// The `<measure …>…</measure>` bodies of a document, in order.
    fn measures(doc: &str) -> Vec<&str> {
        doc.split("<measure ")
            .skip(1)
            .map(|rest| rest.split("</measure>").next().unwrap())
            .collect()
    }

    fn durations(xml: &str) -> u32 {
        xml.split("<duration>")
            .skip(1)
            .map(|rest| rest.split('<').next().unwrap().parse::<u32>().unwrap())
            .sum()
    }

    /// A minimal well-formedness check: every `<x …>` is closed by `</x>` in
    /// order, `<x/>` stands alone, and declarations are skipped.
    fn assert_balanced(doc: &str) {
        let mut stack: Vec<&str> = Vec::new();
        let mut rest = doc;
        while let Some(open) = rest.find('<') {
            let close = open + rest[open..].find('>').expect("unterminated tag");
            let tag = &rest[open + 1..close];
            rest = &rest[close + 1..];
            if tag.starts_with('?') || tag.starts_with('!') {
                continue;
            }
            if let Some(name) = tag.strip_prefix('/') {
                assert_eq!(stack.pop(), Some(name), "mismatched </{name}>");
            } else if !tag.ends_with('/') {
                stack.push(tag.split_whitespace().next().unwrap());
            }
        }
        assert!(stack.is_empty(), "unclosed tags: {stack:?}");
    }

    #[test]
    fn a_single_quarter_note_document() {
        let doc = xml(&[note(60, 0, Q)]);
        for needle in [
            "<divisions>8</divisions>",
            "<fifths>0</fifths>",
            "<mode>major</mode>",
            "<beats>4</beats>",
            "<beat-type>4</beat-type>",
            "<staves>2</staves>",
            "<clef number=\"1\"><sign>G</sign><line>2</line></clef>",
            "<clef number=\"2\"><sign>F</sign><line>4</line></clef>",
            "<software>RockCraft</software>",
            "<part-name>Piano</part-name>",
            "<rest measure=\"yes\"/>",
            "<work-title>Test</work-title>",
        ] {
            assert!(doc.contains(needle), "missing {needle}");
        }
        let notes = note_blocks(&doc);
        let c4 = notes[0];
        assert!(c4.contains("<step>C</step><octave>4</octave>"));
        assert!(c4.contains("<duration>8</duration>"));
        assert!(c4.contains("<type>quarter</type>"));
        assert!(c4.contains("<voice>1</voice>") && c4.contains("<staff>1</staff>"));
        // The bar rest on staff 2 carries the bar length and no <type>.
        let bar_rest = notes.last().unwrap();
        assert!(bar_rest.contains("<duration>32</duration>"));
        assert!(!bar_rest.contains("<type>"));
        assert!(bar_rest.contains("<voice>5</voice>") && bar_rest.contains("<staff>2</staff>"));
    }

    #[test]
    fn only_later_chord_notes_carry_the_chord_tag() {
        let doc = xml(&[note(60, 0, Q), note(64, 0, Q), note(67, 0, Q)]);
        let flags: Vec<bool> = note_blocks(&doc)[..3]
            .iter()
            .map(|n| n.contains("<chord/>"))
            .collect();
        assert_eq!(flags, [false, true, true]);
    }

    #[test]
    fn a_tie_is_written_for_sound_and_for_display() {
        let doc = xml(&[note(60, 3 * Q, 2 * Q)]);
        let bars = measures(&doc);
        assert!(bars[0].contains("<tie type=\"start\"/>"));
        assert!(bars[0].contains("<tied type=\"start\"/>"));
        assert!(bars[1].contains("<tie type=\"stop\"/>"));
        assert!(bars[1].contains("<tied type=\"stop\"/>"));
        assert!(!bars[0].contains("type=\"stop\""));
        assert!(!bars[1].contains("type=\"start\""));
    }

    #[test]
    fn alter_and_accidental() {
        let doc = xml(&[note(61, 0, Q)]);
        let sharp = note_blocks(&doc)[0];
        assert!(sharp.contains("<step>C</step><alter>1</alter><octave>4</octave>"));
        assert!(sharp.contains("<accidental>sharp</accidental>"));

        // B♭ in F major: altered by the key, so no printed accidental.
        let f_major = Some(Key {
            root_pc: 5,
            scale: Scale::Major,
        });
        let doc = xml_in(&[note(70, 0, Q)], &[], f_major, "Test");
        assert!(doc.contains("<fifths>-1</fifths>"));
        let flat = note_blocks(&doc)[0];
        assert!(flat.contains("<step>B</step><alter>-1</alter><octave>4</octave>"));
        assert!(!flat.contains("<accidental>"));
    }

    #[test]
    fn dots_and_tempo_marks() {
        let doc = xml(&[note(60, Q, 3 * Q)]);
        let dotted = note_blocks(&doc)
            .into_iter()
            .find(|n| n.contains("<step>C</step>"))
            .unwrap();
        assert!(dotted.contains("<type>half</type>") && dotted.contains("<dot/>"));

        // Bars 3 and 4 are faster (1.5 s = 160 BPM).
        let starts = [0, 2_000_000, 4_000_000, 5_500_000, 7_000_000];
        let notes: Vec<Note> = starts[..4].iter().map(|&t| note(60, t, 300_000)).collect();
        let doc = xml_in(&notes, &starts, None, "Test");
        let bars = measures(&doc);
        assert!(bars[0].contains("<sound tempo=\"120\"/>"));
        assert!(bars[0].contains("<per-minute>120</per-minute>"));
        assert!(!bars[1].contains("<sound"));
        assert!(bars[2].contains("<sound tempo=\"160\"/>"));
        assert!(!bars[3].contains("<sound"));
    }

    #[test]
    fn the_title_is_escaped() {
        let doc = xml_in(
            &[note(60, 0, Q)],
            &[],
            None,
            "Salt & <Pepper> \"live\" 'take'",
        );
        assert!(doc.contains(
            "<work-title>Salt &amp; &lt;Pepper&gt; &quot;live&quot; &apos;take&apos;</work-title>"
        ));
        assert_balanced(&doc);
    }

    /// A multi-bar, two-staff piece with chords, ties, rests and fingers.
    fn busy_piece() -> Vec<Note> {
        let fingered = |pitch, start, dur, finger| Note {
            finger: Finger::new(finger),
            ..note(pitch, start, dur)
        };
        vec![
            fingered(60, 0, Q, 1),
            fingered(64, 0, Q, 3),
            note(67, Q + Q / 2, 2 * Q),
            note(48, 0, 4 * Q),
            fingered(72, 3 * Q, 3 * Q, 5), // tied over the bar line
            note(43, 4 * Q, Q / 2),
            note(45, 5 * Q, Q / 4),
            note(61, 9 * Q, Q),
        ]
    }

    #[test]
    fn a_busy_score_is_well_formed() {
        assert_balanced(&xml(&busy_piece()));
        assert_balanced(&xml(&[]));
    }

    #[test]
    fn each_staff_fills_every_bar_and_backup_rewinds_it() {
        let doc = xml(&busy_piece());
        let bars = measures(&doc);
        assert!(bars.len() >= 3);
        for (i, bar) in bars.iter().enumerate() {
            let (staff1, rest) = bar.split_once("<backup>").expect("one backup per bar");
            let (backup, staff2) = rest.split_once("</backup>").unwrap();
            assert_eq!(durations(backup), 32, "bar {i}: backup is the full bar");
            // Chord notes share their chord's time: count one per chord.
            let sum = |staff: &str| -> u32 {
                note_blocks(staff)
                    .iter()
                    .filter(|n| !n.contains("<chord/>"))
                    .map(|n| durations(n))
                    .sum()
            };
            assert_eq!(sum(staff1), 32, "bar {i}: staff 1");
            assert_eq!(sum(staff2), 32, "bar {i}: staff 2");
            assert!(!staff1.contains("<staff>2</staff>"));
            assert!(!staff2.contains("<staff>1</staff>"));
        }
    }

    #[test]
    fn a_finger_is_written_once_inside_notations() {
        let doc = xml(&busy_piece());
        assert_eq!(doc.matches("<fingering>").count(), 3);
        // C4 and E4 of the opening chord each carry their own.
        let notes = note_blocks(&doc);
        assert!(notes[0].contains("<technical><fingering>1</fingering></technical>"));
        assert!(notes[1].contains("<technical><fingering>3</fingering></technical>"));
        // The tied C5: its first piece has <tied> then <technical> in one
        // <notations>; the continuation has no fingering.
        let c5: Vec<&str> = notes
            .into_iter()
            .filter(|n| n.contains("<step>C</step><octave>5</octave>"))
            .collect();
        assert_eq!(c5.len(), 2);
        assert_eq!(c5[0].matches("<notations>").count(), 1);
        let tied = c5[0].find("<tied type=\"start\"/>").unwrap();
        let technical = c5[0].find("<technical>").unwrap();
        assert!(tied < technical, "schema order: tied before technical");
        assert!(!c5[1].contains("<fingering>"));
    }

    #[test]
    fn a_piece_without_fingers_writes_no_technical_marks() {
        let doc = xml(&[note(60, 0, Q), note(64, 3 * Q, 2 * Q)]);
        assert!(!doc.contains("<technical>"));
        // An untied, unfingered note has no <notations> at all.
        assert!(!note_blocks(&doc)[0].contains("<notations>"));
    }

    #[test]
    fn the_extension_is_added_only_when_missing() {
        let p = |s: &str| with_musicxml_extension(Path::new(s));
        assert_eq!(p("out/song"), PathBuf::from("out/song.musicxml"));
        assert_eq!(p("out/song.musicxml"), PathBuf::from("out/song.musicxml"));
        assert_eq!(p("out/song.xml"), PathBuf::from("out/song.xml"));
    }
}
