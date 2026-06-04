// song.js — shared domain: 88-key geometry + a small original sample song.
// Ported from crates/core (events.rs) and crates/tui (keyboard.rs) so the
// prototypes use the exact same model the Rust engine produces.
(function () {
  const LOWEST = 21;   // A0
  const HIGHEST = 108; // C8
  const NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

  const isBlack = (n) => [1, 3, 6, 8, 10].includes(n % 12);
  const pitchClass = (n) => n % 12;
  const noteName = (n) => NAMES[n % 12] + (Math.floor(n / 12) - 1);

  // Column geometry for a keyboard `boardW` px wide. Mirrors keyboard.rs:
  // 52 white keys evenly fill the width; black keys straddle the boundary.
  function keyLayout(boardW) {
    const whiteW = boardW / 52;
    const whites = [];
    let wi = 0;
    for (let n = LOWEST; n <= HIGHEST; n++) {
      if (!isBlack(n)) { whites.push({ note: n, x: wi * whiteW, w: whiteW, wi }); wi++; }
    }
    const blackW = whiteW * 0.6;
    const blacks = [];
    for (let n = LOWEST; n <= HIGHEST; n++) {
      if (isBlack(n)) {
        const below = whites.find((k) => k.note === n - 1); // black's n-1 is always white
        blacks.push({ note: n, x: below.x + below.w - blackW / 2, w: blackW });
      }
    }
    const byNote = {};
    whites.forEach((k) => (byNote[k.note] = { cx: k.x + k.w / 2, w: k.w, black: false }));
    blacks.forEach((k) => (byNote[k.note] = { cx: k.x + k.w / 2, w: k.w, black: true }));
    return { whites, blacks, whiteW, blackW, byNote };
  }

  // ── A short original loop: "Ember Lantern", A-minor, 120 BPM ───────────────
  // Left hand = sustained bass (root + fifth); right hand = flowing 8th-note
  // arpeggio + a held top-line melody. vi–IV–I–V (Am–F–C–G), 4 bars, seamless.
  const BEAT = 500;            // ms (120 BPM)
  const BAR = BEAT * 4;        // 2000 ms
  const BARS = 4;
  const LOOP = BAR * BARS;     // 8000 ms

  // bass = [root, fifth] (LH) ; rh = three chord tones (RH arpeggio pool)
  const PROG = [
    { name: "Am", bass: [45, 52], rh: [69, 72, 76] }, // A2 E3 | A4 C5 E5
    { name: "F",  bass: [41, 48], rh: [65, 69, 72] }, // F2 C3 | F4 A4 C5
    { name: "C",  bass: [48, 55], rh: [67, 72, 76] }, // C3 G3 | G4 C5 E5
    { name: "G",  bass: [43, 50], rh: [67, 71, 74] }, // G2 D3 | G4 B4 D5
  ];
  const ARP = [0, 1, 2, 1, 0, 1, 2, 1]; // index into chord tones, per 8th

  const notes = [];
  for (let b = 0; b < BARS; b++) {
    const bar0 = b * BAR;
    const ch = PROG[b];
    // LH — two long bass notes (root for beats 1–2, fifth for beats 3–4)
    notes.push({ note: ch.bass[0], start: bar0, end: bar0 + BEAT * 2, hand: "L", vel: 72 });
    notes.push({ note: ch.bass[1], start: bar0 + BEAT * 2, end: bar0 + BEAT * 4, hand: "L", vel: 66 });
    // RH — 8 eighth-note arpeggio
    for (let i = 0; i < 8; i++) {
      const t = bar0 + i * (BEAT / 2);
      notes.push({ note: ch.rh[ARP[i]], start: t, end: t + BEAT / 2 - 45, hand: "R", vel: i % 2 ? 80 : 94 });
    }
    // RH — held top-line melody (octave above the chord's high tone)
    notes.push({ note: ch.rh[2] + 12, start: bar0, end: bar0 + BEAT * 1.5, hand: "R", vel: 104 });
  }

  window.RC = {
    LOWEST, HIGHEST, isBlack, pitchClass, noteName, keyLayout,
    SONG: {
      title: "Ember Lantern",
      artist: "Lyra Vance",
      key: "A minor",
      tempoBpm: 120,
      timeSig: "4/4",
      notes, BEAT, BAR, BARS, LOOP,
      chords: PROG.map((p) => p.name),
    },
  };
})();
