/* prototypes.jsx — three note-highway directions mounted on a DesignCanvas.
   Each owns a HighwayCanvas engine (shared) but wears its own app chrome. */
const { useRef, useEffect, useState } = React;
const SONG = window.RC.SONG;
const { BEAT, BAR, LOOP } = SONG;

// ── shared engine hook ──────────────────────────────────────────────────
function useEngine(config) {
  const canvasRef = useRef(null);
  const engRef = useRef(null);
  const [, force] = useState(0);
  useEffect(() => {
    let eng;
    try {
      eng = new HighwayCanvas(canvasRef.current, config);
      engRef.current = eng;
      window.__engs = (window.__engs || []).concat(eng);
      eng.start();
    } catch (err) {
      window.__engErr = String(err && err.stack || err);
      console.error("engine init failed", err);
    }
    let raf, last = 0;
    const upd = (t) => { if (t - last > 110) { force((x) => x + 1); last = t; } raf = requestAnimationFrame(upd); };
    raf = requestAnimationFrame(upd);
    return () => { cancelAnimationFrame(raf); eng.stop(); };
  }, []);
  return { canvasRef, engRef };
}

function clockOf(engRef) {
  const eng = engRef.current;
  const elapsed = eng ? performance.now() - eng.t0 : 0;
  const now = elapsed % LOOP;
  const bar = Math.floor(now / BAR);
  const beat = Math.floor((now % BAR) / BEAT);
  return { now, bar, beat, chord: SONG.chords[bar], progress: now / LOOP };
}
const fmt = (now) => `0:0${Math.floor(now / 1000)}`;

const Canvas = React.forwardRef((props, ref) => (
  <canvas ref={ref} style={{ width: "100%", height: "100%", display: "block" }} />
));

const Shell = ({ children, bg }) => (
  <div style={{ height: "100%", display: "flex", flexDirection: "column", background: bg, fontFamily: "'Space Grotesk', system-ui, sans-serif" }}>
    {children}
  </div>
);
const CanvasWrap = ({ canvasRef }) => (
  <div style={{ flex: "1 1 auto", minHeight: 0, position: "relative" }}><Canvas ref={canvasRef} /></div>
);

const Dot = ({ c, s = 9 }) => (
  <span style={{ width: s, height: s, borderRadius: 99, background: c, display: "inline-block", boxShadow: `0 0 8px ${c}66` }} />
);

// ── A · Studio ──────────────────────────────────────────────────────────
const cfgStudio = {
  colorMode: "hands", handColors: { L: "#46c4bb", R: "#e0a24c" },
  perspective: 0, glow: 0.16, gridlines: "soft", keyboard: "realistic",
  kbRatio: 0.19, lead: 2600, bg: "#0e0f13", hitLine: "#ffffff",
};
function StudioProto() {
  const { canvasRef, engRef } = useEngine(cfgStudio);
  const ck = clockOf(engRef);
  const [playing, setPlaying] = useState(true);
  const Icon = ({ d, fill = "none" }) => (
    <svg width="13" height="13" viewBox="0 0 24 24" fill={fill} stroke="#cfd2da" strokeWidth="1.8" strokeLinejoin="round"><path d={d} /></svg>
  );
  return (
    <Shell bg={cfgStudio.bg}>
      <div style={{ display: "flex", alignItems: "center", gap: 16, padding: "0 18px", height: 64, borderBottom: "1px solid rgba(255,255,255,0.06)", background: "#15161d", color: "#e9eaf0" }}>
        <div style={{ width: 28, height: 28, borderRadius: 7, background: "linear-gradient(150deg,#2a2c36,#15161d)", border: "1px solid rgba(255,255,255,0.08)", display: "flex", alignItems: "center", justifyContent: "center", gap: 2 }}>
          <span style={{ width: 3, height: 9, background: "#46c4bb", borderRadius: 2 }} />
          <span style={{ width: 3, height: 14, background: "#e0a24c", borderRadius: 2 }} />
        </div>
        <div style={{ minWidth: 0 }}>
          <div style={{ fontSize: 15, fontWeight: 600, letterSpacing: -0.2 }}>{SONG.title}</div>
          <div style={{ fontSize: 10.5, color: "#8a8d99", fontFamily: "'IBM Plex Mono', monospace", marginTop: 1 }}>{SONG.artist} · {SONG.key} · {SONG.tempoBpm} BPM</div>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 12, marginLeft: 18 }}>
          <Icon d="M19 5v14M9 6l-8 6 8 6z" />
          <button onClick={() => setPlaying((p) => !p)} style={{ width: 34, height: 34, borderRadius: 99, border: "none", background: "#46c4bb", display: "flex", alignItems: "center", justifyContent: "center", cursor: "pointer", boxShadow: "0 2px 10px #46c4bb44" }}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="#0e0f13"><path d="M7 5l13 7-13 7z" /></svg>
          </button>
          <Icon d="M5 5v14M15 6l8 6-8 6z" />
        </div>
        <div style={{ flex: 1, display: "flex", alignItems: "center", gap: 10, minWidth: 0 }}>
          <span style={{ fontSize: 11, fontFamily: "'IBM Plex Mono', monospace", color: "#8a8d99" }}>{fmt(ck.now)}</span>
          <div style={{ flex: 1, height: 4, borderRadius: 99, background: "rgba(255,255,255,0.08)", overflow: "hidden" }}>
            <div style={{ width: `${ck.progress * 100}%`, height: "100%", background: "linear-gradient(90deg,#46c4bb,#e0a24c)" }} />
          </div>
          <span style={{ fontSize: 11, fontFamily: "'IBM Plex Mono', monospace", color: "#8a8d99" }}>0:08</span>
        </div>
        <div style={{ display: "flex", gap: 14, fontSize: 10.5, color: "#aab", fontFamily: "'IBM Plex Mono', monospace" }}>
          <span style={{ display: "flex", alignItems: "center", gap: 5 }}><Dot c="#46c4bb" />L</span>
          <span style={{ display: "flex", alignItems: "center", gap: 5 }}><Dot c="#e0a24c" />R</span>
        </div>
      </div>
      <CanvasWrap canvasRef={canvasRef} />
    </Shell>
  );
}

// ── B · Neon Arcade ─────────────────────────────────────────────────────
const cfgArcade = {
  colorMode: "hands", handColors: { L: "#18c8ff", R: "#ff3df0" },
  perspective: 0.46, glow: 1, gridlines: "soft", keyboard: "flat",
  kbRatio: 0.15, lead: 2400, bg: "#06060d", hitLine: "#18c8ff",
  scoring: true, laneTint: "rgba(255,255,255,0.02)", radius: 6, noteGap: 0.12,
};
function ArcadeProto() {
  const { canvasRef, engRef } = useEngine(cfgArcade);
  clockOf(engRef);
  const eng = engRef.current || {};
  const acc = eng.total ? Math.round((eng.hits / eng.total) * 100) : 100;
  const mult = 1 + Math.floor((eng.combo || 0) / 8);
  return (
    <Shell bg={cfgArcade.bg}>
      <div style={{ display: "flex", alignItems: "center", padding: "0 20px", height: 64, background: "linear-gradient(180deg,#0b0b18,#06060d)", borderBottom: "1px solid rgba(24,200,255,0.18)", color: "#dfe6ff" }}>
        <div>
          <div style={{ fontSize: 12, fontWeight: 700, letterSpacing: 2, textTransform: "uppercase" }}>{SONG.title}</div>
          <div style={{ fontSize: 9.5, letterSpacing: 2, color: "#6a78b0", fontFamily: "'IBM Plex Mono', monospace", marginTop: 2 }}>STAGE 1 · {SONG.key.toUpperCase()}</div>
        </div>
        <div style={{ flex: 1 }} />
        <div style={{ textAlign: "center", marginRight: 28 }}>
          <div style={{ fontSize: 26, fontWeight: 700, lineHeight: 1, color: "#ff3df0", textShadow: "0 0 16px #ff3df099", fontFamily: "'Space Grotesk', sans-serif" }}>×{mult}</div>
          <div style={{ fontSize: 9, letterSpacing: 2, color: "#a06ab0", marginTop: 3 }}>{eng.combo || 0} COMBO</div>
        </div>
        <div style={{ textAlign: "center", marginRight: 26 }}>
          <div style={{ fontSize: 14, fontWeight: 600, color: "#18c8ff", fontFamily: "'IBM Plex Mono', monospace", textShadow: "0 0 12px #18c8ff66" }}>{acc}%</div>
          <div style={{ fontSize: 9, letterSpacing: 2, color: "#5a78a0", marginTop: 3 }}>ACCURACY</div>
        </div>
        <div style={{ textAlign: "right", minWidth: 120 }}>
          <div style={{ fontSize: 30, fontWeight: 700, lineHeight: 1, fontFamily: "'IBM Plex Mono', monospace", color: "#fff", textShadow: "0 0 18px #18c8ff77", fontVariantNumeric: "tabular-nums" }}>{String(eng.score || 0).padStart(6, "0")}</div>
          <div style={{ fontSize: 9, letterSpacing: 2, color: "#5a78a0", marginTop: 2 }}>SCORE</div>
        </div>
      </div>
      <CanvasWrap canvasRef={canvasRef} />
    </Shell>
  );
}

// ── C · Spectrum (educational) ──────────────────────────────────────────
const cfgSpectrum = {
  colorMode: "spectrum", perspective: 0, glow: 0, gridlines: "none",
  keyboard: "flat", labels: true, pitchRuler: true, kbRatio: 0.2,
  lead: 3000, bg: "#14151b", hitLine: "#9aa0b8", noteGap: 0.2, radius: 3,
  laneTint: "rgba(255,255,255,0.012)",
};
function SpectrumProto() {
  const { canvasRef, engRef } = useEngine(cfgSpectrum);
  const ck = clockOf(engRef);
  const wheel = Array.from({ length: 12 }, (_, i) => `oklch(0.72 0.16 ${(i * 30 + 8) % 360})`);
  return (
    <Shell bg={cfgSpectrum.bg}>
      <div style={{ display: "flex", alignItems: "center", gap: 18, padding: "0 18px", height: 64, background: "#1b1c24", borderBottom: "1px solid rgba(255,255,255,0.07)", color: "#e7e8ef" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <div style={{ fontSize: 15, fontWeight: 600 }}>{SONG.title}</div>
          <span style={{ fontSize: 11, fontFamily: "'IBM Plex Mono', monospace", padding: "2px 8px", borderRadius: 6, background: "rgba(255,255,255,0.07)", color: "#b9bccb" }}>{SONG.key}</span>
        </div>
        <div style={{ flex: 1 }} />
        <div style={{ textAlign: "center" }}>
          <div style={{ fontSize: 24, fontWeight: 600, fontFamily: "'IBM Plex Mono', monospace", lineHeight: 1, fontVariantNumeric: "tabular-nums" }}>{ck.bar + 1}<span style={{ color: "#6f7280", opacity: 0.5 }}>:</span>{ck.beat + 1}</div>
          <div style={{ fontSize: 9, letterSpacing: 2, color: "#7c7f8e", marginTop: 3 }}>BAR · BEAT</div>
        </div>
        <div style={{ textAlign: "center", minWidth: 54 }}>
          <div style={{ fontSize: 24, fontWeight: 700, lineHeight: 1, color: "oklch(0.8 0.13 150)" }}>{ck.chord}</div>
          <div style={{ fontSize: 9, letterSpacing: 2, color: "#7c7f8e", marginTop: 3 }}>CHORD</div>
        </div>
        <div style={{ display: "flex", flexDirection: "column", alignItems: "flex-end", gap: 6 }}>
          <div style={{ fontSize: 10.5, fontFamily: "'IBM Plex Mono', monospace", color: "#9a9db0" }}>{SONG.tempoBpm} BPM · {SONG.timeSig}</div>
          <div style={{ display: "flex", gap: 2 }}>{wheel.map((c, i) => <span key={i} style={{ width: 8, height: 8, borderRadius: 2, background: c }} />)}</div>
        </div>
      </div>
      <CanvasWrap canvasRef={canvasRef} />
    </Shell>
  );
}

// ── D · Spectrum Live (C's readability + B's dynamics) ──────────────────
const cfgFusion = {
  colorMode: "spectrum", perspective: 0, glow: 0.32, gridlines: "soft",
  keyboard: "flat", labels: true, pitchRuler: true, kbRatio: 0.2, lead: 3000,
  bg: "#0f1016", hitLine: "#aab2d0", noteGap: 0.2, radius: 3, scoring: true,
  laneTint: "rgba(255,255,255,0.012)",
};
function FusionProto() {
  const { canvasRef, engRef } = useEngine(cfgFusion);
  const ck = clockOf(engRef);
  const eng = engRef.current || {};
  const mult = 1 + Math.floor((eng.combo || 0) / 8);
  const wheel = Array.from({ length: 12 }, (_, i) => `oklch(0.72 0.16 ${(i * 30 + 8) % 360})`);
  return (
    <Shell bg={cfgFusion.bg}>
      <div style={{ display: "flex", alignItems: "center", gap: 18, padding: "0 18px", height: 64, background: "#181922", borderBottom: "1px solid rgba(255,255,255,0.07)", color: "#e7e8ef" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <div style={{ fontSize: 15, fontWeight: 600 }}>{SONG.title}</div>
          <span style={{ fontSize: 11, fontFamily: "'IBM Plex Mono', monospace", padding: "2px 8px", borderRadius: 6, background: "rgba(255,255,255,0.07)", color: "#b9bccb" }}>{SONG.key}</span>
          <div style={{ display: "flex", gap: 2, marginLeft: 2 }}>{wheel.map((cc, i) => <span key={i} style={{ width: 7, height: 7, borderRadius: 2, background: cc }} />)}</div>
        </div>
        <div style={{ flex: 1 }} />
        <div style={{ textAlign: "center" }}>
          <div style={{ fontSize: 22, fontWeight: 600, fontFamily: "'IBM Plex Mono', monospace", lineHeight: 1, fontVariantNumeric: "tabular-nums" }}>{ck.bar + 1}<span style={{ color: "#6f7280", opacity: 0.5 }}>:</span>{ck.beat + 1}</div>
          <div style={{ fontSize: 8.5, letterSpacing: 2, color: "#7c7f8e", marginTop: 3 }}>BAR · BEAT</div>
        </div>
        <div style={{ textAlign: "center", minWidth: 50 }}>
          <div style={{ fontSize: 22, fontWeight: 700, lineHeight: 1, color: "oklch(0.8 0.13 150)" }}>{ck.chord}</div>
          <div style={{ fontSize: 8.5, letterSpacing: 2, color: "#7c7f8e", marginTop: 3 }}>CHORD</div>
        </div>
        <div style={{ width: 1, height: 30, background: "rgba(255,255,255,0.08)" }} />
        <div style={{ textAlign: "center" }}>
          <div style={{ fontSize: 20, fontWeight: 700, lineHeight: 1, color: "#67e3c4", textShadow: "0 0 12px #67e3c455" }}>×{mult}</div>
          <div style={{ fontSize: 8.5, letterSpacing: 2, color: "#5f9b8c", marginTop: 3 }}>{eng.combo || 0} COMBO</div>
        </div>
        <div style={{ textAlign: "right", minWidth: 96 }}>
          <div style={{ fontSize: 24, fontWeight: 700, lineHeight: 1, fontFamily: "'IBM Plex Mono', monospace", color: "#fff", fontVariantNumeric: "tabular-nums" }}>{String(eng.score || 0).padStart(6, "0")}</div>
          <div style={{ fontSize: 8.5, letterSpacing: 2, color: "#7c7f8e", marginTop: 2 }}>SCORE</div>
        </div>
      </div>
      <CanvasWrap canvasRef={canvasRef} />
    </Shell>
  );
}

// ── Context / brief card ────────────────────────────────────────────────
function Brief() {
  const Row = ({ k, title, body, c }) => (
    <div style={{ display: "flex", gap: 12, alignItems: "flex-start" }}>
      <div style={{ width: 26, height: 26, borderRadius: 7, background: c, flex: "0 0 auto", display: "flex", alignItems: "center", justifyContent: "center", fontWeight: 700, fontSize: 13, color: "#0e0f13" }}>{k}</div>
      <div>
        <div style={{ fontSize: 13.5, fontWeight: 600, color: "#e9eaf0" }}>{title}</div>
        <div style={{ fontSize: 12, color: "#9498a6", lineHeight: 1.5, marginTop: 2 }}>{body}</div>
      </div>
    </div>
  );
  return (
    <div style={{ height: "100%", background: "#101118", padding: "26px 30px", fontFamily: "'Space Grotesk', system-ui, sans-serif", display: "flex", flexDirection: "column", gap: 18 }}>
      <div>
        <div style={{ fontSize: 11, letterSpacing: 3, color: "#5e6273", textTransform: "uppercase", fontFamily: "'IBM Plex Mono', monospace" }}>RockCraft · Tauri HTML frontend</div>
        <div style={{ fontSize: 24, fontWeight: 700, color: "#f1f2f7", marginTop: 6, letterSpacing: -0.3 }}>Note highway &amp; keyboard — three directions</div>
        <div style={{ fontSize: 12.5, color: "#9498a6", marginTop: 6, lineHeight: 1.55, maxWidth: 760 }}>
          All three share one engine driven by the real <code style={{ color: "#c9b8a0", fontFamily: "'IBM Plex Mono', monospace" }}>core</code> model: 88-key geometry from <code style={{ color: "#c9b8a0", fontFamily: "'IBM Plex Mono', monospace" }}>keyboard.rs</code> and falling <code style={{ color: "#c9b8a0", fontFamily: "'IBM Plex Mono', monospace" }}>NoteSpan</code>s projected exactly like <code style={{ color: "#c9b8a0", fontFamily: "'IBM Plex Mono', monospace" }}>highway.rs</code> (now = hit-line, now+lead = top). Sample loop is an original 8-second piece, “Ember Lantern.” Notes fall down to the keyboard, matching the TUI.
        </div>
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr 1fr 1fr", gap: 18 }}>
        <Row k="A" c="#46c4bb" title="Studio" body="Clean studio app. Two-hand color (L teal / R amber), realistic shaded keys, soft beat grid, transport + progress." />
        <Row k="B" c="#ff3df0" title="Neon Arcade" body="Rocksmith energy: tapered perspective lane, neon glow, live score / combo / accuracy HUD with judgment popups." />
        <Row k="C" c="oklch(0.78 0.15 150)" title="Spectrum" body="Learner's tool: notes colored by pitch class with note-name labels, octave ruler, bar:beat + chord readout. Flat, calm." />
        <Row k="D" c="#67e3c4" title="Spectrum Live" body="C's readable pitch lanes + labels, with B's dynamics: per-key flashes, spark bursts, centered Great / Perfect judgment." />
      </div>
      <div style={{ marginTop: "auto", fontSize: 11, color: "#6b6f7e", fontFamily: "'IBM Plex Mono', monospace", borderTop: "1px solid rgba(255,255,255,0.06)", paddingTop: 12 }}>
        Mix &amp; match — perspective, glow, color logic, keyboard realism and HUD are independent knobs. Scoring values in B are simulated for the mock.
      </div>
    </div>
  );
}

// ── mount ───────────────────────────────────────────────────────────────
const { DesignCanvas, DCSection, DCArtboard } = window;
function App() {
  return (
    <DesignCanvas>
      <DCSection id="brief" title="Brief" subtitle="Shared model · what's mocked">
        <DCArtboard id="brief" label="Context" width={1080} height={300}><Brief /></DCArtboard>
      </DCSection>
      <DCSection id="protos" title="Note highway + keyboard" subtitle="Three animated directions — same song, same geometry">
        <DCArtboard id="studio" label="A · Studio" width={1080} height={600}><StudioProto /></DCArtboard>
        <DCArtboard id="arcade" label="B · Neon Arcade" width={1080} height={600}><ArcadeProto /></DCArtboard>
        <DCArtboard id="spectrum" label="C · Spectrum" width={1080} height={600}><SpectrumProto /></DCArtboard>
        <DCArtboard id="fusion" label="D · Spectrum Live" width={1080} height={600}><FusionProto /></DCArtboard>
      </DCSection>
    </DesignCanvas>
  );
}
ReactDOM.createRoot(document.getElementById("root")).render(<App />);
