// WSL-side driver for the backing-movie scenario against an ALREADY-RUNNING
// Tauri app on the Windows host (control socket on ws://127.0.0.1:9001, reachable
// from WSL under mirrored networking). Mirrors docs/BACKING-MOVIE-SCENARIO.md
// but does NOT spawn the app — it attaches to the one already running.
//
// Usage: node scripts/drive-backing-movie.mjs
//   ADDR=127.0.0.1:9001            control socket (default)
//   MOVIE=E:\\path\\to\\movie.mp4  Windows path to the backing movie
//   BUNDLE_ROOT=/mnt/e/projects/RockCraft   WSL view of the app's cwd
//
// Node >=21 provides a global WebSocket, so no dependencies are needed.

import fs from "node:fs";
import path from "node:path";

const ADDR = process.env.ADDR || "127.0.0.1:9001";
const MOVIE = process.env.MOVIE || "E:\\projects\\RockCraft\\backing-movie-test.mp4";
const BUNDLE_ROOT = process.env.BUNDLE_ROOT || "/mnt/e/projects/RockCraft";
const URL = `ws://${ADDR}`;

function fail(msg) {
  console.error(`\nSCENARIO FAILED: ${msg}`);
  process.exit(1);
}

const ws = new WebSocket(URL);
const pending = new Map(); // id -> {resolve, reject}
let nextResolveAny = null;

ws.addEventListener("message", (ev) => {
  let msg;
  try {
    msg = JSON.parse(ev.data);
  } catch {
    return;
  }
  // Skip unsolicited frames unless someone is explicitly draining.
  if ((msg.type === "hello" || msg.type === "event") && nextResolveAny == null) {
    console.log(`  (banner: ${msg.type})`);
    return;
  }
  if (nextResolveAny) {
    const r = nextResolveAny;
    nextResolveAny = null;
    r(msg);
    return;
  }
  const id = msg.id;
  if (id != null && pending.has(id)) {
    const { resolve } = pending.get(id);
    pending.delete(id);
    resolve(msg);
  }
});

ws.addEventListener("close", () => {
  for (const { reject } of pending.values()) reject(new Error("connection closed"));
  pending.clear();
});
ws.addEventListener("error", (e) => {
  for (const { reject } of pending.values()) reject(new Error(`ws error: ${e.message || e}`));
  pending.clear();
});

function send(req) {
  return new Promise((resolve, reject) => {
    pending.set(req.id, { resolve, reject });
    ws.send(JSON.stringify(req));
    setTimeout(() => {
      if (pending.has(req.id)) {
        pending.delete(req.id);
        reject(new Error(`timeout waiting for id ${req.id} (${req.action || req.command || req.what})`));
      }
    }, 15000);
  });
}

async function action(id, name, params) {
  const r = await send({ type: "run_action", id, action: name, params });
  if (r.type === "err") throw new Error(`action ${name} failed: ${r.error}`);
  return r;
}
async function host(id, name, params) {
  const r = await send({ type: "run_host_command", id, command: name, params });
  if (r.type === "err") throw new Error(`host ${name} failed: ${r.error}`);
  return r.value;
}
async function query(id, what) {
  const r = await send({ type: "query", id, what });
  if (r.type === "err") throw new Error(`query ${what} failed: ${r.error}`);
  return r;
}

await new Promise((res, rej) => {
  ws.addEventListener("open", res, { once: true });
  ws.addEventListener("error", () => rej(new Error(`cannot connect to ${URL}`)), { once: true });
});
console.log(`• connected to ${URL}`);

try {
  // Beat 0 — discover vocabulary.
  const help = await query(0, "Help");
  const hostCmds = (help.host_commands || []).map((c) => c.name);
  for (const need of ["attach_video", "query_video", "save_bundle", "play_load", "app_quit"]) {
    if (!hostCmds.includes(need)) fail(`help missing host command \`${need}\``);
  }
  console.log("• query Help lists attach_video / play_load / app_quit ✓");

  // Beat 1 — attach the movie as backing.
  const v = await host(1, "attach_video", { path: MOVIE, offset_us: -100000 });
  console.log(`• attach_video → ${JSON.stringify(v)}`);
  const v2 = await host(2, "query_video", {});
  if (v2.path !== MOVIE) fail(`query_video echoed wrong path: ${JSON.stringify(v2)}`);

  // Beat 2 — author 4 ascending notes.
  const melody = [[60, 0], [62, 4], [64, 8], [65, 12]];
  let i = 0;
  for (const [pitch, step] of melody) {
    await action(100 + i, "set_cursor", { pitch, step });
    await action(200 + i, "add_note", {});
    i++;
  }
  const st = await query(3, "State");
  const n = (st.state?.notes || []).length;
  if (n !== melody.length) fail(`expected ${melody.length} notes, got ${n}`);
  console.log(`• authored ${n} notes ✓`);

  // Beat 3 — unsaved edits.
  const dirty1 = await host(4, "query_dirty", {});
  if (dirty1 !== true) fail(`expected dirty=true, got ${dirty1}`);
  console.log("• query_dirty → true ✓");

  // Beat 4 — save the bundle.
  const saved = await host(5, "save_bundle", { dest: { kind: "quick_save" } });
  const dirRel = saved.dir;
  if (!dirRel) fail(`save_bundle returned no dir: ${JSON.stringify(saved)}`);
  // The Windows app returns a backslash relative path; normalise to POSIX so the
  // WSL-side filesystem check resolves it under the shared /mnt/e mount.
  const bundleDir = path.posix.join(BUNDLE_ROOT, dirRel.replaceAll("\\", "/"));
  console.log(`• save_bundle → ${bundleDir}`);

  // Beat 5 — dirty cleared + bundle on disk with video in meta.
  const dirty2 = await host(6, "query_dirty", {});
  if (dirty2 !== false) fail(`expected dirty=false after save, got ${dirty2}`);
  const mid = path.join(bundleDir, "song.mid");
  const metaPath = path.join(bundleDir, "meta.json");
  if (!fs.existsSync(mid)) fail(`song.mid not written at ${mid}`);
  const meta = JSON.parse(fs.readFileSync(metaPath, "utf8"));
  if (meta.video == null) fail(`meta.json has no video block: ${JSON.stringify(meta)}`);
  console.log("• bundle persisted: song.mid + meta.json (with video) ✓");

  // Beat 6 — load in playback mode; notes + movie come back.
  const winDir = dirRel.replaceAll("/", "\\");
  const info = await host(7, "play_load", { dir: winDir });
  const played = (info.notes || []).length;
  if (played !== melody.length) fail(`play_load surfaced ${played} notes, expected ${melody.length}`);
  if (info.video == null) fail(`play_load lost the movie backing: ${JSON.stringify(info)}`);
  console.log(`• play_load → ${played} notes + movie backing ✓`);

  // Beat 7 — finish the take.
  const summary = await host(8, "play_finish", {});
  console.log(`• play_finish → ${JSON.stringify(summary)}`);

  // Beat 8 — quit. Reply may not flush before teardown; close also counts.
  console.log("• app_quit → shutting the game down");
  try {
    nextResolveAny = () => {};
    ws.send(JSON.stringify({ type: "run_host_command", id: 9, command: "app_quit", params: {} }));
    await new Promise((r) => setTimeout(r, 1500));
  } catch {}

  console.log("\nSCENARIO OK");
  process.exit(0);
} catch (e) {
  fail(e.message || String(e));
}
