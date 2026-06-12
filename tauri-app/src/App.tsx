import { createSignal, Match, Switch, type JSX } from "solid-js";
import { Router, useRouter } from "./shell/Router";
import { Placeholder } from "./shell/Placeholder";
import { MenuScreen } from "./screens/menu/MenuScreen";
import { HighwayScreen } from "./screens/highway/HighwayScreen";
import { RecordScreen } from "./screens/record/RecordScreen";
import { LibraryScreen } from "./screens/library/LibraryScreen";
import { EditScreen } from "./screens/edit/EditScreen";
import { UrlInput } from "./screens/import/UrlInput";
import { ImportingScreen } from "./screens/import/ImportingScreen";
import { importStart } from "./ipc/bridge";

function AppShell(): JSX.Element {
  const { screen, navigate } = useRouter();
  /** Bundle dir from the most recent import, used to highlight in library. */
  const [importBundleDir, setImportBundleDir] = createSignal<
    string | undefined
  >(undefined);

  function handleUrlSubmit(url: string): void {
    navigate({ kind: "importing" });
    setImportBundleDir(undefined);
    importStart({ kind: "Url", value: url }).catch(() => {
      navigate({ kind: "menu" });
    });
  }

  return (
    <Switch>
      <Match when={screen().kind === "menu"}>
        <MenuScreen />
      </Match>
      <Match when={screen().kind === "record"}>
        <RecordScreen />
      </Match>
      <Match when={screen().kind === "play"}>
        <HighwayScreen />
      </Match>
      <Match when={screen().kind === "library"}>
        <LibraryScreen />
      </Match>
      {/* Composer edit screen (#164): piano-roll grid + keymap over the IPC
          bridge. Reachable from the menu via "Compose (new)" / "Edit last
          recording". */}
      <Match when={screen().kind === "edit"}>
        <EditScreen />
      </Match>
      {/* URL input modal — only shown when a fetch command is configured. */}
      <Match when={screen().kind === "url-input"}>
        <UrlInput onSubmit={handleUrlSubmit} />
      </Match>
      {/* Import progress screen. */}
      <Match when={screen().kind === "importing"}>
        <ImportingScreen bundleDir={importBundleDir()} />
      </Match>
      <Match when={screen().kind !== "menu"}>
        <Placeholder screen={screen()} />
      </Match>
    </Switch>
  );
}

export default function App(): JSX.Element {
  return (
    <Router>
      <AppShell />
    </Router>
  );
}
