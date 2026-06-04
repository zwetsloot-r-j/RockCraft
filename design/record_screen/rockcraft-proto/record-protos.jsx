/* record-protos.jsx — four record-screen directions on a DesignCanvas.
   All share the RecordCanvas engine (rising capture of the Ember Lantern take)
   but wear different visualizations + control-density philosophies.
   Shared chrome comes from window.recUI; geometry/song from window.RC. */
const RUI = window.recUI;
const RNAME = window.RC.noteName;
const bbOf = (ms) => `${Math.floor(ms / RUI.BAR) + 1}.${Math.floor((ms % RUI.BAR) / RUI.BEAT) + 1}`;
const DEVICE = "Casio PX-S3100";

// count-in overlay (plays once on mount)
function CountIn() {
  const [n, setN] = React.useState(3);
  React.useEffect(() => {
    const ids = [setTimeout(() => setN(2), 650), setTimeout(() => setN(1), 1300), setTimeout(() => setN(0), 1950)];
    return () => ids.forEach(clearTimeout);
  }, []);
  if (n === 0) return null;
  return (
    <div style={{ position: "absolute", inset: 0, display: "flex", alignItems: "center", justifyContent: "center", pointerEvents: "none", zIndex: 6 }}>
      <div key={n} style={{ fontFamily: RUI.DISP, fontWeight: 700, fontSize: 150, color: "#fff", textShadow: "0 0 60px rgba(255,77,87,0.6)", animation: "countpop .65s ease-out", opacity: 0.92 }}>{n}</div>
    </div>
  );
}

// ── A · TAKE — rising ribbons, minimalist (tools hidden until summoned) ─────
function RecTake() {
  const { useRec, rclock, tc, RecShell, CanvasWrap, RoundBtn, ToolBtn, Chip, Meter, RecDot, Icon, Dot, MONO, DISP, REC, OK } = RUI;
  const { useState } = React;
  const { canvasRef, engRef } = useRec({ viz: "ribbons", glow: 0.6, window: 4400, kbRatio: 0.16 });
  const ck = rclock(engRef);
  const [paused, setPaused] = useState(false);
  const [drawer, setDrawer] = useState(false);
  React.useEffect(() => { if (engRef.current) engRef.current.cfg.paused = paused; }, [paused]);
  return (
    <RecShell>
      <div style={{ display: "flex", alignItems: "center", gap: 14, padding: "0 18px", height: 50, borderBottom: "1px solid rgba(255,255,255,0.05)" }}>
        <RecDot paused={paused} />
        <div style={{ fontFamily: MONO, fontSize: 21, fontWeight: 500, letterSpacing: 0.5, fontVariantNumeric: "tabular-nums" }}>{tc(ck.now)}</div>
        <div style={{ flex: 1 }} />
        <Chip c="#cfd3df" bg="rgba(0,0,0,0.24)"><Dot c={OK} s={6} /> {DEVICE}</Chip>
        <Chip>TAKE 03</Chip>
        <Meter level={ck.level} />
      </div>
      <CanvasWrap canvasRef={canvasRef}>
        <CountIn />
        {/* minimalist floating transport */}
        <div style={{ position: "absolute", left: "50%", bottom: 108, transform: "translateX(-50%)", display: "flex", alignItems: "center", gap: 12, padding: "8px 12px", borderRadius: 99, background: "rgba(16,17,24,0.7)", backdropFilter: "blur(10px)", border: "1px solid rgba(255,255,255,0.08)" }}>
          <RoundBtn icon="rewind" fill="fill" size={30} title="Rewind" />
          <RoundBtn icon={paused ? "play" : "stop"} fill="fill" color={paused ? OK : REC} bg={paused ? "rgba(91,231,196,0.14)" : "rgba(255,77,87,0.16)"} glow size={42} onClick={() => setPaused((p) => !p)} title={paused ? "Resume" : "Stop"} />
          <RoundBtn icon="loop" size={30} title="Loop" />
          <span style={{ width: 1, height: 22, background: "rgba(255,255,255,0.1)" }} />
          <button onClick={() => setDrawer((d) => !d)} style={{ display: "flex", alignItems: "center", gap: 6, border: "none", background: "transparent", color: "#aeb2c2", cursor: "pointer", fontFamily: MONO, fontSize: 11, padding: "4px 6px" }}>
            EDIT <Icon d={drawer ? "chevDown" : "chevUp"} size={14} />
          </button>
        </div>
        {/* edit drawer — hidden by default (the "tuck it away" philosophy) */}
        {drawer && (
          <div style={{ position: "absolute", left: "50%", bottom: 152, transform: "translateX(-50%)", display: "flex", gap: 4, padding: 6, borderRadius: 12, background: "rgba(20,21,30,0.92)", backdropFilter: "blur(12px)", border: "1px solid rgba(255,255,255,0.09)", boxShadow: "0 12px 40px rgba(0,0,0,0.5)", animation: "drawerup .18s ease-out" }}>
            <ToolBtn icon="scissors" label="Trim" />
            <ToolBtn icon="magnet" label="Snap" />
            <ToolBtn icon="pencil" label="Edit" />
            <ToolBtn icon="play" label="Replay" />
            <ToolBtn icon="trash" label="Delete" danger />
          </div>
        )}
      </CanvasWrap>
    </RecShell>
  );
}

// ── B · CONSOLE — rising ribbons, everything visible (power user) ───────────
function RecConsole() {
  const { useRec, rclock, tc, RecShell, CanvasWrap, RoundBtn, ToolBtn, Chip, Toggle, Seg, Meter, RecDot, VRule, Dot, MONO, DISP, REC, OK } = RUI;
  const { useState } = React;
  const { canvasRef, engRef } = useRec({ viz: "ribbons", glow: 0.55, window: 4000, kbRatio: 0.15 });
  const ck = rclock(engRef);
  const [metro, setMetro] = useState(true);
  const [count, setCount] = useState(true);
  const [snap, setSnap] = useState("1/16");
  const [take, setTake] = useState("T3");
  return (
    <RecShell bg="#0e0f15">
      {/* top status bar */}
      <div style={{ display: "flex", alignItems: "center", gap: 12, padding: "0 16px", height: 56, background: "#15161f", borderBottom: "1px solid rgba(255,255,255,0.06)" }}>
        <div style={{ width: 26, height: 26, borderRadius: 6, background: "linear-gradient(150deg,#2a2c36,#15161d)", border: "1px solid rgba(255,255,255,0.08)", display: "flex", alignItems: "center", justifyContent: "center", gap: 2 }}>
          <span style={{ width: 3, height: 8, background: "oklch(0.72 0.16 150)", borderRadius: 2 }} />
          <span style={{ width: 3, height: 13, background: "oklch(0.72 0.16 30)", borderRadius: 2 }} />
        </div>
        <div>
          <div style={{ fontSize: 14, fontWeight: 600, letterSpacing: -0.2 }}>{RUI.RSONG.title} <span style={{ color: "#7c8094", fontWeight: 400 }}>· Take 03</span></div>
          <div style={{ fontSize: 10, color: "#7c8094", fontFamily: MONO, marginTop: 1 }}>{RUI.RSONG.key} · {RUI.RSONG.tempoBpm} BPM · {RUI.RSONG.timeSig}</div>
        </div>
        <VRule />
        <RecDot />
        <div style={{ fontFamily: MONO, fontSize: 16, fontWeight: 500, fontVariantNumeric: "tabular-nums" }}>{tc(ck.now)}</div>
        <Chip mono>BAR {ck.bar + 1}.{ck.beat + 1}</Chip>
        <Chip c={"oklch(0.82 0.13 150)"}>{ck.chord}</Chip>
        <div style={{ flex: 1 }} />
        <Toggle active={metro} onClick={() => setMetro((m) => !m)} title="Metronome"><RUI.Icon d="metro" size={13} stroke={metro ? OK : "#6e7282"} /> {RUI.RSONG.tempoBpm}</Toggle>
        <Toggle active={count} onClick={() => setCount((c) => !c)} title="Count-in">COUNT 1</Toggle>
        <Chip c="#cfd3df" bg="rgba(0,0,0,0.24)"><Dot c={OK} s={6} /> MIDI · {DEVICE}</Chip>
        <Meter level={ck.level} />
      </div>

      <CanvasWrap canvasRef={canvasRef} />

      {/* full toolbar — all controls available at once */}
      <div style={{ display: "flex", alignItems: "center", gap: 10, padding: "0 14px", height: 58, background: "#15161f", borderTop: "1px solid rgba(255,255,255,0.06)" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
          <RoundBtn icon="rewind" fill="fill" size={32} title="Rewind" />
          <RoundBtn icon="stop" fill="fill" color={REC} bg="rgba(255,77,87,0.16)" glow size={38} title="Stop" />
          <RoundBtn icon="play" fill="fill" size={32} title="Play take" />
          <RoundBtn icon="loop" size={32} title="Loop" />
        </div>
        <VRule />
        <div style={{ display: "flex", alignItems: "center", gap: 2 }}>
          <ToolBtn icon="scissors" label="Trim" />
          <ToolBtn icon="trash" label="Delete" danger />
          <ToolBtn icon="magnet" label="Quantize" />
          <ToolBtn icon="target" label="Punch-in" />
          <ToolBtn icon="pencil" label="Edit" />
          <ToolBtn icon="undo" label="Undo" />
          <ToolBtn icon="redo" label="Redo" />
        </div>
        <div style={{ flex: 1 }} />
        <Seg label="SNAP" options={["1/8", "1/16", "1/32", "OFF"]} value={snap} onChange={setSnap} />
        <VRule />
        <Seg label="TAKE" options={["T1", "T2", "T3"]} value={take} onChange={setTake} />
      </div>
    </RecShell>
  );
}

// ── C · SCORE — live stylized sheet music + ribbon band (combination) ───────
function RecScore() {
  const { useRec, rclock, tc, RecShell, CanvasWrap, RoundBtn, Chip, Seg, RecDot, VRule, Dot, Icon, MONO, DISP, REC, OK } = RUI;
  const { useState } = React;
  const { canvasRef, engRef } = useRec({ viz: "ribbons+staff", glow: 0.5, window: 2400, kbRatio: 0.15 });
  const ck = rclock(engRef);
  const [spelling, setSpelling] = useState("♯");
  const [clef, setClef] = useState("Grand");
  const sel = engRef.current && engRef.current.sel;
  return (
    <RecShell bg="#101119">
      <div style={{ display: "flex", alignItems: "center", gap: 12, padding: "0 16px", height: 54, background: "#181a24", borderBottom: "1px solid rgba(255,255,255,0.06)" }}>
        <RecDot />
        <div style={{ fontFamily: MONO, fontSize: 15, fontWeight: 500, fontVariantNumeric: "tabular-nums" }}>{tc(ck.now)}</div>
        <VRule />
        <div style={{ fontSize: 14, fontWeight: 600 }}>{RUI.RSONG.title}</div>
        <Chip>{RUI.RSONG.key}</Chip>
        <Chip c={"oklch(0.82 0.13 150)"}>{ck.chord}</Chip>
        <div style={{ flex: 1 }} />
        <Seg label="CLEF" options={["Grand", "Treble"]} value={clef} onChange={setClef} />
        <Seg label="SPELL" options={["♯", "♭"]} value={spelling} onChange={setSpelling} />
        <Chip c="#cfd3df" bg="rgba(0,0,0,0.24)"><Dot c={OK} s={6} /> {DEVICE}</Chip>
      </div>

      <CanvasWrap canvasRef={canvasRef}>
        {/* selected-note inspector — editing happens on the notation */}
        {sel && (
          <div style={{ position: "absolute", top: 14, right: 14, width: 188, padding: "12px 13px", borderRadius: 11, background: "rgba(20,22,31,0.9)", backdropFilter: "blur(12px)", border: "1px solid rgba(255,255,255,0.09)", boxShadow: "0 10px 30px rgba(0,0,0,0.4)" }}>
            <div style={{ fontSize: 9, letterSpacing: 2, color: "#7c8094", fontFamily: MONO, marginBottom: 8 }}>SELECTED NOTE</div>
            <div style={{ display: "flex", alignItems: "center", gap: 9, marginBottom: 10 }}>
              <span style={{ width: 16, height: 12, borderRadius: 99, background: `oklch(0.74 0.16 ${(sel.note % 12 * 30 + 8) % 360})`, boxShadow: `0 0 10px oklch(0.74 0.16 ${(sel.note % 12 * 30 + 8) % 360})`, transform: "rotate(-20deg)" }} />
              <span style={{ fontSize: 22, fontWeight: 700, fontFamily: DISP }}>{RNAME(sel.note)}</span>
            </div>
            {[["Beat", bbOf(sel.start)], ["Length", "♩ quarter"], ["Velocity", sel.vel], ["Hand", sel.hand === "R" ? "Right" : "Left"]].map(([k, v]) => (
              <div key={k} style={{ display: "flex", justifyContent: "space-between", fontSize: 11.5, padding: "3px 0", color: "#aeb2c2" }}>
                <span style={{ color: "#7c8094" }}>{k}</span><span style={{ fontFamily: MONO }}>{v}</span>
              </div>
            ))}
            <div style={{ display: "flex", gap: 6, marginTop: 11 }}>
              <button style={{ flex: 1, border: "1px solid rgba(255,255,255,0.12)", background: "rgba(255,255,255,0.05)", color: "#dfe2ea", borderRadius: 7, padding: "6px 0", fontSize: 11, fontFamily: DISP, cursor: "pointer" }}>Nudge</button>
              <button style={{ flex: 1, border: `1px solid ${OK}44`, background: `${OK}1a`, color: OK, borderRadius: 7, padding: "6px 0", fontSize: 11, fontFamily: DISP, cursor: "pointer" }}>Snap</button>
            </div>
          </div>
        )}
        {/* floating transport */}
        <div style={{ position: "absolute", left: "50%", bottom: 14, transform: "translateX(-50%)", display: "flex", alignItems: "center", gap: 10, padding: "7px 11px", borderRadius: 99, background: "rgba(16,17,24,0.72)", backdropFilter: "blur(10px)", border: "1px solid rgba(255,255,255,0.08)" }}>
          <RoundBtn icon="rewind" fill="fill" size={28} />
          <RoundBtn icon="stop" fill="fill" color={REC} bg="rgba(255,77,87,0.16)" glow size={36} />
          <RoundBtn icon="play" fill="fill" size={28} />
          <RoundBtn icon="loop" size={28} />
        </div>
      </CanvasWrap>
    </RecShell>
  );
}

// ── D · ROLL — DAW horizontal piano-roll editor (deep edit) ─────────────────
function RecRoll() {
  const { useRec, rclock, tc, RecShell, CanvasWrap, RoundBtn, ToolBtn, Chip, Seg, RecDot, VRule, Dot, Icon, MONO, DISP, REC, OK } = RUI;
  const { useState } = React;
  const { canvasRef, engRef } = useRec({ viz: "roll", glow: 0.5, kbRatio: 0.13 });
  const ck = rclock(engRef);
  const [tool, setTool] = useState("Select");
  const [snap, setSnap] = useState("1/16");
  const sel = engRef.current && engRef.current.sel;
  const Step = ({ children }) => (
    <span style={{ display: "inline-flex", alignItems: "center", gap: 4, fontFamily: MONO, fontSize: 12, color: "#dfe2ea", background: "rgba(0,0,0,0.3)", borderRadius: 6, padding: "3px 5px" }}>
      <button style={{ border: "none", background: "transparent", color: "#8a8e9c", cursor: "pointer", padding: 0, display: "flex" }}><Icon d="minus" size={11} /></button>
      <span style={{ minWidth: 34, textAlign: "center" }}>{children}</span>
      <button style={{ border: "none", background: "transparent", color: "#8a8e9c", cursor: "pointer", padding: 0, display: "flex" }}><Icon d="plus" size={11} /></button>
    </span>
  );
  return (
    <RecShell bg="#0d0e14">
      <div style={{ display: "flex", alignItems: "center", gap: 10, padding: "0 14px", height: 50, background: "#14151e", borderBottom: "1px solid rgba(255,255,255,0.06)" }}>
        <RecDot />
        <div style={{ fontFamily: MONO, fontSize: 15, fontWeight: 500, fontVariantNumeric: "tabular-nums" }}>{tc(ck.now)}</div>
        <Chip mono>BAR {ck.bar + 1}.{ck.beat + 1}</Chip>
        <VRule />
        <div style={{ display: "flex", alignItems: "center", gap: 5 }}>
          <RoundBtn icon="rewind" fill="fill" size={28} />
          <RoundBtn icon="stop" fill="fill" color={REC} bg="rgba(255,77,87,0.16)" glow size={34} />
          <RoundBtn icon="play" fill="fill" size={28} />
          <RoundBtn icon="loop" size={28} />
        </div>
        <div style={{ flex: 1 }} />
        <div style={{ fontSize: 14, fontWeight: 600 }}>{RUI.RSONG.title} <span style={{ color: "#7c8094", fontWeight: 400, fontSize: 12 }}>· Take 03</span></div>
        <Chip c="#cfd3df" bg="rgba(0,0,0,0.24)"><Dot c={OK} s={6} /> {DEVICE}</Chip>
      </div>

      <CanvasWrap canvasRef={canvasRef} />

      {/* edit dock: tool palette + selected-note inspector (the deep edit) */}
      <div style={{ display: "flex", alignItems: "center", gap: 10, padding: "0 12px", height: 56, background: "#14151e", borderTop: "1px solid rgba(255,255,255,0.06)" }}>
        <div style={{ display: "flex", gap: 1 }}>
          {[["cursor", "Select"], ["pencil", "Draw"], ["scissors", "Trim"], ["magnet", "Quantize"], ["target", "Punch"], ["trash", "Delete"]].map(([ic, t]) => (
            <ToolBtn key={t} icon={ic} label={t} active={tool === t} danger={t === "Delete"} onClick={() => setTool(t)} />
          ))}
        </div>
        <VRule />
        <Seg label="SNAP" options={["1/8", "1/16", "1/32"]} value={snap} onChange={setSnap} />
        <div style={{ flex: 1 }} />
        {sel && (
          <div style={{ display: "flex", alignItems: "center", gap: 12, padding: "6px 12px", borderRadius: 9, background: "rgba(0,0,0,0.25)", border: "1px solid rgba(255,255,255,0.07)" }}>
            <span style={{ display: "flex", alignItems: "center", gap: 7 }}>
              <span style={{ width: 12, height: 12, borderRadius: 3, background: `oklch(0.7 0.16 ${(sel.note % 12 * 30 + 8) % 360})`, boxShadow: `0 0 8px oklch(0.7 0.16 ${(sel.note % 12 * 30 + 8) % 360})` }} />
              <span style={{ fontSize: 9, letterSpacing: 1.5, color: "#7c8094", fontFamily: MONO }}>NOTE</span>
            </span>
            <label style={{ display: "flex", alignItems: "center", gap: 6 }}><span style={{ fontSize: 9, letterSpacing: 1, color: "#7c8094", fontFamily: MONO }}>PITCH</span><Step>{RNAME(sel.note)}</Step></label>
            <label style={{ display: "flex", alignItems: "center", gap: 6 }}><span style={{ fontSize: 9, letterSpacing: 1, color: "#7c8094", fontFamily: MONO }}>START</span><Step>{bbOf(sel.start)}</Step></label>
            <label style={{ display: "flex", alignItems: "center", gap: 6 }}><span style={{ fontSize: 9, letterSpacing: 1, color: "#7c8094", fontFamily: MONO }}>VEL</span><Step>{sel.vel}</Step></label>
            <button style={{ border: `1px solid ${OK}44`, background: `${OK}1a`, color: OK, borderRadius: 7, padding: "7px 12px", fontSize: 11.5, fontFamily: DISP, cursor: "pointer", display: "flex", alignItems: "center", gap: 5 }}><Icon d="magnet" size={13} stroke={OK} /> Quantize</button>
          </div>
        )}
      </div>
    </RecShell>
  );
}

// ── E · STUDIO — B's full menu bars + C's live notation (combination) ───────
function RecStudioFull() {
  const { useRec, rclock, tc, RecShell, CanvasWrap, RoundBtn, ToolBtn, Chip, Toggle, Seg, Meter, RecDot, VRule, Dot, Icon, MONO, DISP, REC, OK } = RUI;
  const { useState } = React;
  const { canvasRef, engRef } = useRec({ viz: "ribbons+staff", glow: 0.5, window: 2200, kbRatio: 0.14 });
  const ck = rclock(engRef);
  const [metro, setMetro] = useState(true);
  const [count, setCount] = useState(true);
  const [snap, setSnap] = useState("1/16");
  const [clef, setClef] = useState("Grand");
  const [spelling, setSpelling] = useState("♯");
  const sel = engRef.current && engRef.current.sel;
  const hue = sel ? (sel.note % 12 * 30 + 8) % 360 : 0;
  return (
    <RecShell bg="#101119">
      {/* ── top status bar (from B · Console) ── */}
      <div style={{ display: "flex", alignItems: "center", gap: 12, padding: "0 16px", height: 56, background: "#181a24", borderBottom: "1px solid rgba(255,255,255,0.06)" }}>
        <div style={{ width: 26, height: 26, borderRadius: 6, background: "linear-gradient(150deg,#2a2c36,#15161d)", border: "1px solid rgba(255,255,255,0.08)", display: "flex", alignItems: "center", justifyContent: "center", gap: 2 }}>
          <span style={{ width: 3, height: 8, background: "oklch(0.72 0.16 150)", borderRadius: 2 }} />
          <span style={{ width: 3, height: 13, background: "oklch(0.72 0.16 30)", borderRadius: 2 }} />
        </div>
        <div>
          <div style={{ fontSize: 14, fontWeight: 600, letterSpacing: -0.2 }}>{RUI.RSONG.title} <span style={{ color: "#7c8094", fontWeight: 400 }}>· Take 03</span></div>
          <div style={{ fontSize: 10, color: "#7c8094", fontFamily: MONO, marginTop: 1 }}>{RUI.RSONG.key} · {RUI.RSONG.tempoBpm} BPM · {RUI.RSONG.timeSig}</div>
        </div>
        <VRule />
        <RecDot />
        <div style={{ fontFamily: MONO, fontSize: 16, fontWeight: 500, fontVariantNumeric: "tabular-nums" }}>{tc(ck.now)}</div>
        <Chip mono>BAR {ck.bar + 1}.{ck.beat + 1}</Chip>
        <Chip c={"oklch(0.82 0.13 150)"}>{ck.chord}</Chip>
        <div style={{ flex: 1 }} />
        <Toggle active={metro} onClick={() => setMetro((m) => !m)} title="Metronome"><Icon d="metro" size={13} stroke={metro ? OK : "#6e7282"} /> {RUI.RSONG.tempoBpm}</Toggle>
        <Toggle active={count} onClick={() => setCount((c) => !c)} title="Count-in">COUNT 1</Toggle>
        <Chip c="#cfd3df" bg="rgba(0,0,0,0.24)"><Dot c={OK} s={6} /> MIDI · {DEVICE}</Chip>
        <Meter level={ck.level} />
      </div>

      <CanvasWrap canvasRef={canvasRef}>
        {/* selected-note inspector (from C · Score) */}
        {sel && (
          <div style={{ position: "absolute", top: 12, right: 12, width: 184, padding: "11px 13px", borderRadius: 11, background: "rgba(20,22,31,0.9)", backdropFilter: "blur(12px)", border: "1px solid rgba(255,255,255,0.09)", boxShadow: "0 10px 30px rgba(0,0,0,0.4)" }}>
            <div style={{ fontSize: 9, letterSpacing: 2, color: "#7c8094", fontFamily: MONO, marginBottom: 8 }}>SELECTED NOTE</div>
            <div style={{ display: "flex", alignItems: "center", gap: 9, marginBottom: 9 }}>
              <span style={{ width: 16, height: 12, borderRadius: 99, background: `oklch(0.74 0.16 ${hue})`, boxShadow: `0 0 10px oklch(0.74 0.16 ${hue})`, transform: "rotate(-20deg)" }} />
              <span style={{ fontSize: 22, fontWeight: 700, fontFamily: DISP }}>{RNAME(sel.note)}</span>
            </div>
            {[["Beat", bbOf(sel.start)], ["Length", "♩ quarter"], ["Velocity", sel.vel]].map(([k, v]) => (
              <div key={k} style={{ display: "flex", justifyContent: "space-between", fontSize: 11.5, padding: "3px 0", color: "#aeb2c2" }}>
                <span style={{ color: "#7c8094" }}>{k}</span><span style={{ fontFamily: MONO }}>{v}</span>
              </div>
            ))}
            <div style={{ display: "flex", gap: 6, marginTop: 10 }}>
              <button style={{ flex: 1, border: "1px solid rgba(255,255,255,0.12)", background: "rgba(255,255,255,0.05)", color: "#dfe2ea", borderRadius: 7, padding: "6px 0", fontSize: 11, fontFamily: DISP, cursor: "pointer" }}>Nudge</button>
              <button style={{ flex: 1, border: `1px solid ${OK}44`, background: `${OK}1a`, color: OK, borderRadius: 7, padding: "6px 0", fontSize: 11, fontFamily: DISP, cursor: "pointer" }}>Snap</button>
            </div>
          </div>
        )}
      </CanvasWrap>

      {/* ── bottom toolbar (from B · Console) + notation view controls (from C) ── */}
      <div style={{ display: "flex", alignItems: "center", gap: 10, padding: "0 14px", height: 58, background: "#181a24", borderTop: "1px solid rgba(255,255,255,0.06)" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
          <RoundBtn icon="rewind" fill="fill" size={32} title="Rewind" />
          <RoundBtn icon="stop" fill="fill" color={REC} bg="rgba(255,77,87,0.16)" glow size={38} title="Stop" />
          <RoundBtn icon="play" fill="fill" size={32} title="Play take" />
          <RoundBtn icon="loop" size={32} title="Loop" />
        </div>
        <VRule />
        <div style={{ display: "flex", alignItems: "center", gap: 2 }}>
          <ToolBtn icon="scissors" label="Trim" />
          <ToolBtn icon="trash" label="Delete" danger />
          <ToolBtn icon="magnet" label="Quantize" />
          <ToolBtn icon="target" label="Punch-in" />
          <ToolBtn icon="undo" label="Undo" />
          <ToolBtn icon="redo" label="Redo" />
        </div>
        <div style={{ flex: 1 }} />
        <Seg label="CLEF" options={["Grand", "Treble"]} value={clef} onChange={setClef} />
        <Seg label="SPELL" options={["♯", "♭"]} value={spelling} onChange={setSpelling} />
        <VRule />
        <Seg label="SNAP" options={["1/8", "1/16", "1/32"]} value={snap} onChange={setSnap} />
      </div>
    </RecShell>
  );
}

// ── brief card ──────────────────────────────────────────────────────────────
function RecBrief() {
  const { MONO, DISP, REC, OK } = RUI;
  const Row = ({ k, c, title, body }) => (
    <div style={{ display: "flex", gap: 12, alignItems: "flex-start" }}>
      <div style={{ width: 26, height: 26, borderRadius: 7, background: c, flex: "0 0 auto", display: "flex", alignItems: "center", justifyContent: "center", fontWeight: 700, fontSize: 13, color: "#0e0f13" }}>{k}</div>
      <div>
        <div style={{ fontSize: 13.5, fontWeight: 600, color: "#e9eaf0" }}>{title}</div>
        <div style={{ fontSize: 12, color: "#9498a6", lineHeight: 1.5, marginTop: 2 }}>{body}</div>
      </div>
    </div>
  );
  return (
    <div style={{ height: "100%", background: "#101118", padding: "26px 30px", fontFamily: DISP, display: "flex", flexDirection: "column", gap: 18 }}>
      <div>
        <div style={{ fontSize: 11, letterSpacing: 3, color: "#5e6273", textTransform: "uppercase", fontFamily: MONO }}>RockCraft · Record screen</div>
        <div style={{ fontSize: 24, fontWeight: 700, color: "#f1f2f7", marginTop: 6, letterSpacing: -0.3 }}>Capturing a take — four directions</div>
        <div style={{ fontSize: 12.5, color: "#9498a6", marginTop: 6, lineHeight: 1.55, maxWidth: 820 }}>
          The keyboard stays at the bottom like the Note Highway, but here notes are <b style={{ color: "#c9b8a0" }}>emitted as you play</b> and rise away — a live record of the take. Same Spectrum colour (hue by pitch class), same <code style={{ color: "#c9b8a0", fontFamily: MONO }}>Ember Lantern</code> loop, recorded from a MIDI keyboard. Two axes vary across the four: <b style={{ color: "#cdd0db" }}>how the take is visualized</b>, and <b style={{ color: "#cdd0db" }}>how dense the controls are</b>.
        </div>
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr 1fr 1fr", gap: 18 }}>
        <Row k="A" c={REC} title="Take · minimal" body="Rising note ribbons. One record orb + timecode; trim / snap / edit stay tucked in a drawer until summoned." />
        <Row k="B" c="oklch(0.72 0.16 30)" title="Console · maximal" body="Same ribbons, but every control on deck: full transport, trim / delete / quantize / punch-in, metronome, MIDI, snap & take." />
        <Row k="C" c="oklch(0.78 0.15 280)" title="Score · notation" body="Ribbons crystallize into a stylized grand staff as you play. Pick a note to nudge or snap it. Combination view." />
        <Row k="D" c={OK} title="Roll · editor" body="DAW horizontal piano-roll. Drag, trim, quantize and punch-in with snap-to-grid + a per-note inspector." />
      </div>
      <div style={{ marginTop: "auto", fontSize: 11, color: "#6b6f7e", fontFamily: MONO, borderTop: "1px solid rgba(255,255,255,0.06)", paddingTop: 12 }}>
        Playback loops + simulates a live MIDI performance. Transport / edit controls are mocked affordances — toggles & segmented pickers respond; drag-edit is illustrative.
      </div>
    </div>
  );
}

// ── mount ───────────────────────────────────────────────────────────────────
function RecApp() {
  const { DesignCanvas, DCSection, DCArtboard } = window;
  return (
    <DesignCanvas>
      <DCSection id="brief" title="Brief" subtitle="Record screen · shared model">
        <DCArtboard id="brief" label="Context" width={1100} height={300}><RecBrief /></DCArtboard>
      </DCSection>
      <DCSection id="viz" title="Recording visualization" subtitle="How the take is shown — rising ribbons, notation, piano-roll">
        <DCArtboard id="take" label="A · Take (minimal)" width={1080} height={600}><RecTake /></DCArtboard>
        <DCArtboard id="console" label="B · Console (maximal)" width={1080} height={600}><RecConsole /></DCArtboard>
        <DCArtboard id="score" label="C · Score (notation)" width={1080} height={600}><RecScore /></DCArtboard>
        <DCArtboard id="roll" label="D · Roll (editor)" width={1080} height={600}><RecRoll /></DCArtboard>
      </DCSection>
      <DCSection id="combo" title="Combined direction" subtitle="B's full menu bars wrapping C's live notation">
        <DCArtboard id="studio" label="E · Studio (full + notation)" width={1080} height={600}><RecStudioFull /></DCArtboard>
      </DCSection>
    </DesignCanvas>
  );
}
ReactDOM.createRoot(document.getElementById("rec-root")).render(<RecApp />);
