// SheetView.tsx — the piece as sheet music, drawn inside the editor (M22-C).
//
// A full-editor overlay: it asks the backend for the piece as MusicXML
// (`query_score`, the document `export_score` writes) and draws it with
// OpenSheetMusicDisplay. The library is large, so it is imported on first use
// rather than with the editor. The view is a snapshot of the piece as it was
// when the overlay opened; key handling lives in EditScreen (see sheet.ts).

import { createEffect, createSignal, on, onCleanup, onMount, Show, type JSX } from "solid-js";
import type { OpenSheetMusicDisplay } from "opensheetmusicdisplay";

import { editQueryScore } from "../../ipc/bridge";

interface Props {
  /** Zoom factor (1 = the library's natural size); changing it re-renders. */
  zoom: number;
  /** A short message to show in the bar, e.g. the result of an export. */
  message?: string | null;
}

/** Print only the sheet: everything else in the window is hidden on paper. */
const PRINT_CSS = `
@media print {
  body * { visibility: hidden !important; }
  .rc-sheet-paper, .rc-sheet-paper * { visibility: visible !important; }
  .rc-sheet-overlay, .rc-sheet-scroll {
    position: static !important; overflow: visible !important;
    height: auto !important; background: #fff !important; padding: 0 !important;
  }
  .rc-sheet-paper { box-shadow: none !important; margin: 0 !important; max-width: none !important; }
  .rc-sheet-bar { display: none !important; }
}`;

export function SheetView(props: Props): JSX.Element {
  let paper!: HTMLDivElement;
  let osmd: OpenSheetMusicDisplay | null = null;
  const [status, setStatus] = createSignal<string | null>("loading…");
  const [title, setTitle] = createSignal("");

  onMount(() => {
    void (async () => {
      try {
        const doc = await editQueryScore();
        setTitle(doc.title);
        setStatus("drawing…");
        const { OpenSheetMusicDisplay } = await import("opensheetmusicdisplay");
        osmd = new OpenSheetMusicDisplay(paper, {
          backend: "svg",
          autoResize: true,
          // Our export writes no beams; let the renderer group the flags.
          autoBeam: true,
          drawTitle: true,
          drawPartNames: false,
          drawFingerings: true,
          drawMeasureNumbers: true,
          drawMeasureNumbersOnlyAtSystemStart: true,
        });
        await osmd.load(doc.musicxml);
        osmd.zoom = props.zoom;
        osmd.render();
        setStatus(null);
      } catch (err) {
        const detail = err instanceof Error ? err.message : String(err);
        setStatus(
          detail === "nothing to export"
            ? "This piece has no notes yet."
            : `Could not draw the sheet: ${detail}`,
        );
      }
    })();
  });

  // Zoom changes re-render the already loaded score.
  createEffect(
    on(
      () => props.zoom,
      (zoom) => {
        if (osmd === null || status() !== null) return;
        osmd.zoom = zoom;
        osmd.render();
      },
      { defer: true },
    ),
  );

  onCleanup(() => {
    osmd?.clear();
    osmd = null;
  });

  return (
    <div
      class="rc-sheet-overlay"
      style={{
        position: "absolute",
        inset: "0",
        "z-index": 110,
        display: "flex",
        "flex-direction": "column",
        background: "#e9e6df",
        color: "#1d1c1a",
      }}
    >
      <style>{PRINT_CSS}</style>
      <div
        class="rc-sheet-bar"
        style={{
          display: "flex",
          "align-items": "center",
          gap: "18px",
          padding: "8px 16px",
          background: "#15161d",
          color: "#d7d9e0",
          "font-size": "12px",
          "flex-shrink": 0,
        }}
      >
        <strong style={{ "font-size": "13px" }}>Sheet view</strong>
        <span style={{ opacity: 0.8 }}>{title()}</span>
        <span style={{ "margin-left": "auto", opacity: 0.75 }}>
          {Math.round(props.zoom * 100)}% · [+/-] zoom · [e] export MusicXML · [p] print · [Esc] close
        </span>
        <Show when={props.message}>
          <span style={{ color: "#f5c542", "font-weight": 600 }}>{props.message}</span>
        </Show>
      </div>
      <div
        class="rc-sheet-scroll"
        style={{ flex: "1", overflow: "auto", padding: "20px 16px 40px" }}
      >
        <Show when={status()}>
          <div style={{ "text-align": "center", padding: "48px 0", color: "#6b675f" }}>
            {status()}
          </div>
        </Show>
        <div
          ref={paper}
          class="rc-sheet-paper"
          style={{
            background: "#ffffff",
            margin: "0 auto",
            "max-width": "1100px",
            padding: "12px",
            "box-shadow": "0 2px 12px rgba(0,0,0,0.18)",
            // Hidden until drawn, but kept in the layout: OSMD needs its width.
            visibility: status() === null ? "visible" : "hidden",
          }}
        />
      </div>
    </div>
  );
}
