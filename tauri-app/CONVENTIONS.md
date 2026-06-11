# tauri-app SolidJS conventions

The `tauri-app/` frontend is **SolidJS** (JSX, fine-grained reactivity — no
vDOM). These rules are referenced by every later frontend spec; follow them
when porting the React-flavoured design prototypes in
`design/*/rockcraft-proto/*.jsx`.

1. **Components run once.** There is no re-render mental model; reactivity
   lives in signals. Never destructure props (it breaks reactivity); read
   `props.x` directly or use `splitProps`.
2. **State**: `createSignal` for scalars, `createStore` for object state
   (e.g. the `ComposerSnapshot` mirror in #161). No external state libraries.
3. **Lifecycle**: `onMount` / `onCleanup` for engine start/stop, event
   listeners, and intervals — every `listen()` from Tauri gets its unlisten in
   `onCleanup`.
4. **Refs**: `let el!: HTMLCanvasElement;` + `ref={el}`.
5. **Control flow**: `<Show>` / `<For>` / `<Switch>` instead of ternaries and
   `.map` for lists/conditional screens.
6. **Styles**: inline style objects use kebab-case string keys
   (`"flex-direction": "column"`), unlike React's camelCase.
7. **Porting the design prototypes**: `useState` → `createSignal`,
   `useRef` → plain `let` + `ref`, `useEffect(…, [])` →
   `onMount`/`onCleanup`, `useReducer`-as-tick → a `frame` signal bumped by
   the throttle loop; values read in JSX become accessor calls (`metro()`).
