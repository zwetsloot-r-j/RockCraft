import {
  createContext,
  createSignal,
  onCleanup,
  onMount,
  useContext,
  type JSX,
} from "solid-js";
import type { Screen } from "./screens";

// ── Context ────────────────────────────────────────────────────────────────

interface RouterContext {
  screen: () => Screen;
  navigate: (s: Screen) => void;
}

const RouterCtx = createContext<RouterContext>();

export function useRouter(): RouterContext {
  const ctx = useContext(RouterCtx);
  if (!ctx) throw new Error("useRouter must be used inside <Router>");
  return ctx;
}

// ── Router component ───────────────────────────────────────────────────────

interface RouterProps {
  children: JSX.Element;
}

export function Router(props: RouterProps): JSX.Element {
  const [screen, setScreen] = createSignal<Screen>({ kind: "menu" });

  function navigate(s: Screen): void {
    setScreen(s);
  }

  // Global Escape: return to menu from any non-menu screen.
  function onKeydown(e: KeyboardEvent): void {
    if (e.key === "Escape" && screen().kind !== "menu") {
      navigate({ kind: "menu" });
    }
  }

  onMount(() => window.addEventListener("keydown", onKeydown));
  onCleanup(() => window.removeEventListener("keydown", onKeydown));

  return (
    <RouterCtx.Provider value={{ screen, navigate }}>
      {props.children}
    </RouterCtx.Provider>
  );
}
