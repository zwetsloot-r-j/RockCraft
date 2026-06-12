import { Match, Switch, type JSX } from "solid-js";
import { Router, useRouter } from "./shell/Router";
import { Placeholder } from "./shell/Placeholder";
import { MenuScreen } from "./screens/menu/MenuScreen";
import { HighwayScreen } from "./screens/highway/HighwayScreen";
import { RecordScreen } from "./screens/record/RecordScreen";

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
