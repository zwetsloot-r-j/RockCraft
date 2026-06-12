import { createSignal, For, onCleanup, onMount, type JSX } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useRouter } from "../../shell/Router";
import type { Screen } from "../../shell/screens";

// ── Menu items ────────────────────────────────────────────────────────────

interface MenuItem {
  label: string;
  screen: Screen | null; // null = handled inline (Quit)
  disabled?: boolean;
  disabledTitle?: string;
}

const MENU_ITEMS: MenuItem[] = [
  { label: "Record", screen: { kind: "record" } },
  { label: "Play last recording", screen: { kind: "play" } },
  { label: "Compose (new)", screen: { kind: "edit" } },
  { label: "Edit last recording", screen: { kind: "edit" } },
  { label: "Library…", screen: { kind: "library" } },
  { label: "Choose backing track", screen: { kind: "backing-picker" } },
  { label: "Import from video file…", screen: { kind: "video-picker" } },
  {
    label: "Import from URL…",
    screen: { kind: "url-input" },
    disabled: true,
    disabledTitle: "no fetch command configured",
  },
  { label: "Quit", screen: null },
];

// ── Component ─────────────────────────────────────────────────────────────

export function MenuScreen(): JSX.Element {
  const { navigate } = useRouter();
  const [selected, setSelected] = createSignal(0);

  function activate(idx: number): void {
    const item = MENU_ITEMS[idx];
    if (!item || item.disabled) return;
    if (item.screen === null) {
      void getCurrentWindow().close();
    } else {
      navigate(item.screen);
    }
  }

  function moveSelection(delta: number): void {
    const count = MENU_ITEMS.length;
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
        activate(selected());
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
        <For each={MENU_ITEMS}>
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
                onClick={() => activate(idx())}
              >
                {item.label}
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}
