import { Match, Switch, type JSX } from "solid-js";
import { Router, useRouter } from "./shell/Router";
import { Placeholder } from "./shell/Placeholder";
import { MenuScreen } from "./screens/menu/MenuScreen";
import { HighwayScreen } from "./screens/highway/HighwayScreen";
import { RecordScreen } from "./screens/record/RecordScreen";
import { LibraryScreen } from "./screens/library/LibraryScreen";
import { EditScreen } from "./screens/edit/EditScreen";

function AppShell(): JSX.Element {
  const { screen } = useRouter();

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
      {/* Composer edit screen (#164+#165): piano-roll grid + keymap over the
          IPC bridge. Reachable from the menu via "Compose (new)" / "Edit last
          recording", or from the library browser with a bundle `dir` payload. */}
      <Match when={screen().kind === "edit"}>
        {/* Type-narrow to extract the optional `dir` field. */}
        <EditScreen dir={screen().kind === "edit" ? (screen() as { kind: "edit"; dir?: string }).dir : undefined} />
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
