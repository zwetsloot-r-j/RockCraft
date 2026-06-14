import {
  createSignal,
  For,
  onCleanup,
  onMount,
  type JSX,
} from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useRouter } from "../../shell/Router";
import {
  importUrlAvailable,
  importStart,
  latestRecording,
  openVideoFilePicker,
} from "../../ipc/bridge";

// ── Menu item definitions ─────────────────────────────────────────────────

/** A menu item entry (label + action). */
interface MenuItem {
  label: string;
  /** Unique key for keyboard navigation identification. */
  key: string;
  /** True if the item should be hidden (not just disabled). */
  hidden?: boolean;
  disabled?: boolean;
  disabledTitle?: string;
}

// The static part of the menu (order preserved; dynamic items managed below).
//
// M9-A collapsed Record / Compose (new) / Edit last recording into the single
// capture+edit screen: "New piece" opens it empty and armed for recording;
// "Continue last" reopens the most recent bundle in the same screen.
const STATIC_ITEMS: MenuItem[] = [
  { label: "New piece", key: "new-piece" },
  { label: "Continue last", key: "continue-last" },
  { label: "Play last recording", key: "play" },
  { label: "Library…", key: "library" },
  { label: "Choose backing track", key: "backing-picker" },
  { label: "Import from video file…", key: "video-import" },
  { label: "Import from URL…", key: "url-import" },
  { label: "Quit", key: "quit" },
];

// ── Component ─────────────────────────────────────────────────────────────

export function MenuScreen(): JSX.Element {
  const { navigate } = useRouter();
  const [selected, setSelected] = createSignal(0);
  /** Whether a fetch command is configured (controls URL import visibility). */
  const [urlAvailable, setUrlAvailable] = createSignal<boolean>(false);
  /** Transient inline notice (e.g. "no recordings yet"); null when hidden. */
  const [notice, setNotice] = createSignal<string | null>(null);

  let noticeTimer: number | undefined;
  function showNotice(msg: string): void {
    setNotice(msg);
    if (noticeTimer !== undefined) window.clearTimeout(noticeTimer);
    noticeTimer = window.setTimeout(() => setNotice(null), 4000);
  }
  onCleanup(() => {
    if (noticeTimer !== undefined) window.clearTimeout(noticeTimer);
  });

  /**
   * Open the newest bundle on `screen`, or show the no-recordings notice when
   * the library is empty. Shared by "Play last recording" / "Continue last"
   * so both route real bundles (never the retired demo fixture).
   */
  async function openLatest(screen: "play" | "edit"): Promise<void> {
    try {
      const dir = await latestRecording();
      if (dir) navigate({ kind: screen, dir });
      else showNotice("No recordings yet — record or import one first.");
    } catch {
      showNotice("No recordings yet — record or import one first.");
    }
  }

  // Query URL availability on mount.
  onMount(() => {
    importUrlAvailable()
      .then((v) => setUrlAvailable(v))
      .catch(() => {
        /* backend not yet available — leave as false */
      });
  });

  /** Visible items depend on urlAvailable. */
  function visibleItems(): MenuItem[] {
    return STATIC_ITEMS.filter((item) => {
      if (item.key === "url-import" && !urlAvailable()) return false;
      return true;
    });
  }

  async function activate(key: string): Promise<void> {
    switch (key) {
      // New piece: the unified capture+edit screen, empty and armed for
      // recording so a played note lands immediately (M9-A).
      case "new-piece":
        navigate({ kind: "edit", armed: true });
        break;
      // Continue last: reopen the most recent bundle in the same screen.
      case "continue-last":
        await openLatest("edit");
        break;
      case "play":
        await openLatest("play");
        break;
      case "library":
        navigate({ kind: "library" });
        break;
      case "backing-picker":
        navigate({ kind: "backing-picker" });
        break;
      case "video-import": {
        const path = await openVideoFilePicker();
        if (path) {
          navigate({ kind: "importing" });
          importStart({ kind: "File", value: path }).catch(() => {
            // import_start error (e.g. "already running") — navigate back.
            navigate({ kind: "menu" });
          });
        }
        break;
      }
      case "url-import":
        navigate({ kind: "url-input" });
        break;
      case "quit":
        void getCurrentWindow().close();
        break;
    }
  }

  function moveSelection(delta: number): void {
    const items = visibleItems();
    const count = items.length;
    setSelected((s) => ((s + delta) % count + count) % count);
  }

  function onKeydown(e: KeyboardEvent): void {
    switch (e.key) {
      case "ArrowUp":
      case "k":
        e.preventDefault();
        moveSelection(-1);
        break;
      case "ArrowDown":
      case "j":
        e.preventDefault();
        moveSelection(1);
        break;
      case "Enter":
        e.preventDefault();
        void activate(visibleItems()[selected()]?.key ?? "");
        break;
      default:
        break;
    }
  }

  onMount(() => window.addEventListener("keydown", onKeydown));
  onCleanup(() => window.removeEventListener("keydown", onKeydown));

  return (
    <div
      style={{
        height: "100vh",
        display: "flex",
        "flex-direction": "column",
        "align-items": "center",
        "justify-content": "center",
        background: "#0f1016",
        color: "#e7e8ef",
        "font-family": "'Space Grotesk', system-ui, sans-serif",
      }}
    >
      {/* Wordmark */}
      <div
        style={{
          "font-size": "28px",
          "font-weight": 700,
          "letter-spacing": "-0.5px",
          "margin-bottom": "40px",
          color: "#fff",
        }}
      >
        RockCraft
      </div>

      {/* Menu list */}
      <div
        style={{
          display: "flex",
          "flex-direction": "column",
          width: "320px",
          gap: "2px",
        }}
      >
        <For each={visibleItems()}>
          {(item, idx) => {
            const isSelected = () => selected() === idx();
            return (
              <div
                title={item.disabled ? item.disabledTitle : undefined}
                style={{
                  padding: "10px 16px",
                  "border-radius": "8px",
                  cursor: item.disabled ? "not-allowed" : "pointer",
                  background: isSelected()
                    ? "rgba(255,255,255,0.1)"
                    : "transparent",
                  color: item.disabled
                    ? "#4a4d5a"
                    : isSelected()
                      ? "#fff"
                      : "#b0b3c1",
                  "font-size": "15px",
                  "font-weight": isSelected() ? 600 : 400,
                  transition: "background 0.1s, color 0.1s",
                  "user-select": "none",
                  outline: isSelected()
                    ? "1px solid rgba(255,255,255,0.15)"
                    : "none",
                }}
                onMouseEnter={() => setSelected(idx())}
                onClick={() => void activate(item.key)}
              >
                {item.label}
              </div>
            );
          }}
        </For>
      </div>

      {/* Transient inline notice (e.g. empty-library feedback). */}
      {notice() && (
        <div
          style={{
            "margin-top": "20px",
            "max-width": "320px",
            "text-align": "center",
            "font-size": "13px",
            color: "#b0b3c1",
          }}
        >
          {notice()}
        </div>
      )}
    </div>
  );
}
