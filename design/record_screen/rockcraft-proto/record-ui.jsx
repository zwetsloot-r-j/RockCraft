/* record-ui.jsx — shared chrome for the Record-screen prototypes.
   Icons, the RecordCanvas hook, and small transport/toolbar primitives.
   Exported to window so the (separately-transpiled) prototypes file can use
   them. Visual language matches the Note Highway protos: dark studio chrome,
   Space Grotesk + IBM Plex Mono, Spectrum (pitch-class) colour. */
const { useRef, useEffect, useState, useCallback } = React;
const RC = window.RC;
const RSONG = RC.SONG;
const { BEAT, BAR, LOOP } = RSONG;

const MONO = "'IBM Plex Mono', ui-monospace, monospace";
const DISP = "'Space Grotesk', system-ui, sans-serif";
const REC = "#ff4d57";              // record red
const OK = "#5be7c4";               // confirm / armed green

// ── icon set ──────────────────────────────────────────────────────────────
const P = {
  rewind: "M11 5 4 12l7 7zM20 5l-7 7 7 7z",
  forward: "M13 5l7 7-7 7zM4 5l7 7-7 7z",
  play: "M7 4l13 8-13 8z",
  stop: "M6 6h12v12H6z",
  loop: "M17 1l4 4-4 4M3 11V9a4 4 0 014-4h14M7 23l-4-4 4-4M21 13v2a4 4 0 01-4 4H3",
  undo: "M3 8h11a6 6 0 010 12h-3M3 8l4-4M3 8l4 4",
  redo: "M21 8H10a6 6 0 000 12h3M21 8l-4-4M21 8l-4 4",
  scissors: "M6 6l12 12M8.5 8.5L18 6M6 18l4-4M9 6a3 3 0 11-6 0 3 3 0 016 0zM9 18a3 3 0 11-6 0 3 3 0 016 0z",
  trash: "M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14M10 10v6M14 10v6",
  grid: "M4 4h16v16H4zM4 9.3h16M4 14.6h16M9.3 4v16M14.6 4v16",
  magnet: "M6 3v8a6 6 0 0012 0V3h-4v8a2 2 0 01-4 0V3zM6 3H2m12 0h4",
  metro: "M12 3l5.5 17H6.5zM6.5 20h11M12 19l4.5-9",
  target: "M12 2a10 10 0 100 20 10 10 0 000-20zM12 7a5 5 0 100 10 5 5 0 000-10zM12 11a1 1 0 100 2 1 1 0 000-2z",
  pencil: "M4 20l1-4L16 5l3 3L8 19zM14 7l3 3",
  midi: "M5 9a7 7 0 0114 0v3H5zM9 9V6m3 3V5m3 4V6M12 19v3",
  chevDown: "M6 9l6 6 6-6",
  chevUp: "M6 15l6-6 6 6",
  mic: "M12 3a3 3 0 00-3 3v6a3 3 0 006 0V6a3 3 0 00-3-3zM5 11a7 7 0 0014 0M12 18v3",
  cursor: "M5 3l14 7-6 2-2 6z",
  plus: "M12 5v14M5 12h14",
  minus: "M5 12h14",
};
function Icon({ d, size = 16, fill = "none", stroke = "currentColor", w = 1.8 }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill={fill} stroke={fill === "none" ? stroke : "none"}
      strokeWidth={w} strokeLinejoin="round" strokeLinecap="round" style={{ display: "block", flex: "0 0 auto" }}>
      <path d={P[d] || d} />
    </svg>
  );
}

// ── engine hook ────────────────────────────────────────────────────────────
function useRec(cfg) {
  const canvasRef = useRef(null);
  const engRef = useRef(null);
  const [, force] = useState(0);
  useEffect(() => {
    let eng;
    try {
      eng = new RecordCanvas(canvasRef.current, cfg);
      engRef.current = eng;
      (window.__recs = window.__recs || []).push(eng);
      eng.start();
    } catch (e) { window.__recErr = String(e && e.stack || e); console.error("rec init failed", e); }
    let raf, last = 0;
    const upd = (t) => { if (t - last > 90) { force((x) => x + 1); last = t; } raf = requestAnimationFrame(upd); };
    raf = requestAnimationFrame(upd);
    return () => { cancelAnimationFrame(raf); eng && eng.stop(); };
  }, []);
  return { canvasRef, engRef };
}

function rclock(engRef) {
  const eng = engRef.current;
  const now = eng ? eng.tNow : 0;
  const bar = Math.floor(now / BAR);
  const beat = Math.floor((now % BAR) / BEAT);
  return { now, bar, beat, chord: RSONG.chords[bar] || "—", level: eng ? eng.level : 0, progress: now / LOOP };
}
const tc = (ms) => { const s = Math.floor(ms / 1000), cs = Math.floor((ms % 1000) / 10); return `${s}:${String(cs).padStart(2, "0")}`; };

// ── shared bits ────────────────────────────────────────────────────────────
const Canvas = React.forwardRef((props, ref) => (
  <canvas ref={ref} style={{ width: "100%", height: "100%", display: "block" }} />
));
const RecShell = ({ children, bg = "#0f1016" }) => (
  <div style={{ height: "100%", display: "flex", flexDirection: "column", background: bg, fontFamily: DISP, color: "#e7e8ef", position: "relative", overflow: "hidden" }}>{children}</div>
);
const CanvasWrap = ({ canvasRef, children }) => (
  <div style={{ flex: "1 1 auto", minHeight: 0, position: "relative" }}>
    <Canvas ref={canvasRef} />
    {children}
  </div>
);
const Dot = ({ c, s = 9, glow = true }) => (
  <span style={{ width: s, height: s, borderRadius: 99, background: c, display: "inline-block", boxShadow: glow ? `0 0 8px ${c}aa` : "none" }} />
);
const VRule = ({ o = 0.09 }) => <span style={{ width: 1, alignSelf: "stretch", margin: "8px 0", background: `rgba(255,255,255,${o})` }} />;

// round transport button
function RoundBtn({ icon, fill = "none", bg = "rgba(255,255,255,0.06)", color = "#dfe2ea", size = 34, glow, active, onClick, title }) {
  return (
    <button title={title} onClick={onClick} style={{
      width: size, height: size, borderRadius: 99, border: active ? `1px solid ${color}66` : "1px solid rgba(255,255,255,0.08)",
      background: bg, color, display: "flex", alignItems: "center", justifyContent: "center", cursor: "pointer",
      boxShadow: glow ? `0 2px 14px ${color}55` : "none", flex: "0 0 auto", transition: "transform .1s",
    }}>
      <Icon d={icon} fill={fill === "fill" ? color : "none"} stroke={color} size={size * 0.46} />
    </button>
  );
}
// square tool button (toolbars)
function ToolBtn({ icon, label, active, danger, onClick }) {
  const col = danger ? REC : active ? OK : "#c7cad6";
  return (
    <button onClick={onClick} title={label} style={{
      display: "flex", flexDirection: "column", alignItems: "center", gap: 3, padding: "6px 9px",
      border: active ? `1px solid ${col}55` : "1px solid transparent", borderRadius: 8,
      background: active ? `${col}1a` : "transparent", color: col, cursor: "pointer", minWidth: 46,
    }}>
      <Icon d={icon} size={17} stroke={col} />
      {label && <span style={{ fontSize: 9, letterSpacing: 0.3, fontFamily: MONO, color: active ? col : "#8a8e9c" }}>{label}</span>}
    </button>
  );
}
function Chip({ children, c = "#aeb2c2", bg = "rgba(255,255,255,0.06)", mono = true }) {
  return <span style={{ fontSize: 10.5, fontFamily: mono ? MONO : DISP, padding: "3px 8px", borderRadius: 6, background: bg, color: c, display: "inline-flex", alignItems: "center", gap: 6, whiteSpace: "nowrap" }}>{children}</span>;
}
// dark toggle pill — light readable text on a dark fill; green only as an
// accent (border + indicator) so the active state stays legible.
function Toggle({ active, onClick, title, children }) {
  return (
    <button onClick={onClick} title={title} style={{
      display: "inline-flex", alignItems: "center", gap: 6, cursor: "pointer",
      fontFamily: MONO, fontSize: 10.5, padding: "4px 9px", borderRadius: 6,
      background: "rgba(0,0,0,0.24)",
      border: `1px solid ${active ? "rgba(91,231,196,0.5)" : "rgba(255,255,255,0.09)"}`,
      color: active ? "#e7e8ef" : "#6e7282", transition: "border-color .12s, color .12s",
    }}>{children}</button>
  );
}
// segmented control
function Seg({ options, value, onChange, label }) {
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
      {label && <span style={{ fontSize: 9, letterSpacing: 1.5, color: "#7c8094", fontFamily: MONO }}>{label}</span>}
      <div style={{ display: "flex", background: "rgba(0,0,0,0.3)", borderRadius: 7, padding: 2, gap: 2 }}>
        {options.map((o) => (
          <button key={o} onClick={() => onChange(o)} style={{
            border: "none", cursor: "pointer", padding: "3px 9px", borderRadius: 5, fontFamily: MONO, fontSize: 11,
            background: value === o ? "rgba(255,255,255,0.12)" : "transparent", color: value === o ? "#fff" : "#888c9c",
          }}>{o}</button>
        ))}
      </div>
    </div>
  );
}
// level meter (vertical bars)
function Meter({ level, bars = 14, w = 56 }) {
  return (
    <div style={{ display: "flex", alignItems: "flex-end", gap: 1.5, height: 18, width: w }}>
      {Array.from({ length: bars }, (_, i) => {
        const on = level * bars > i;
        const c = i / bars > 0.78 ? REC : i / bars > 0.6 ? "#ffd166" : OK;
        return <span key={i} style={{ flex: 1, height: `${30 + (i / bars) * 70}%`, borderRadius: 1, background: on ? c : "rgba(255,255,255,0.09)", boxShadow: on ? `0 0 5px ${c}88` : "none", transition: "background .08s" }} />;
      })}
    </div>
  );
}
// blinking REC indicator
function RecDot({ on = true, paused }) {
  return (
    <span style={{ display: "inline-flex", alignItems: "center", gap: 7 }}>
      <span style={{ position: "relative", width: 11, height: 11 }}>
        <span style={{ position: "absolute", inset: 0, borderRadius: 99, background: paused ? "#ffb020" : REC, animation: on && !paused ? "recpulse 1.1s ease-in-out infinite" : "none", boxShadow: `0 0 10px ${paused ? "#ffb020" : REC}` }} />
      </span>
      <span style={{ fontSize: 11, fontWeight: 700, letterSpacing: 2, color: paused ? "#ffb020" : REC, fontFamily: MONO }}>{paused ? "PAUSED" : "REC"}</span>
    </span>
  );
}

// keyframes (one-time)
if (typeof document !== "undefined" && !document.getElementById("rec-kf")) {
  const s = document.createElement("style"); s.id = "rec-kf";
  s.textContent = `@keyframes recpulse{0%,100%{opacity:1;transform:scale(1)}50%{opacity:.35;transform:scale(.82)}}
  @keyframes countpop{0%{opacity:0;transform:scale(1.5)}18%{opacity:1;transform:scale(1)}82%{opacity:1}100%{opacity:0;transform:scale(.7)}}
  @keyframes drawerup{from{transform:translateY(100%)}to{transform:translateY(0)}}`;
  document.head.appendChild(s);
}

Object.assign(window, {
  recUI: {
    useRec, rclock, tc, Icon, P, Canvas, RecShell, CanvasWrap, Dot, VRule,
    RoundBtn, ToolBtn, Chip, Toggle, Seg, Meter, RecDot, MONO, DISP, REC, OK, RSONG, BEAT, BAR, LOOP,
  },
});
