// Live spectator view and replay player.

import { Renderer, frameAt, type Trail } from "./render.js";
import type { Frame, MapView, RunDetail, RunSummary, TurnLog, WsEvent } from "./types.js";

interface Clip {
  frames: Frame[];
  /** Frame to show once the clip has finished (start state after a reset). */
  after: Frame;
  turn: TurnLog;
}

interface RunView {
  summary: RunSummary;
  map: MapView | null;
  /** Turns of the current map, in order. */
  turns: TurnLog[];
  /** Turns whose clips have been played (index into `turns`). */
  played: number;
  queue: Clip[];
  clipTime: number;
  idle: Frame | null;
}

interface ReplayState {
  detail: RunDetail;
  mapIdx: number;
  cursor: number;
  playing: boolean;
}

const $ = <T extends HTMLElement>(id: string): T => {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
};

const canvas = $<HTMLCanvasElement>("maze");
const renderer = new Renderer(canvas);
const runs = new Map<string, RunView>();
let selectedId: string | null = null;
let replay: ReplayState | null = null;
let recentRuns: RunSummary[] = [];
let lastTick = performance.now();
let wsStatus = "connecting";

const showShortest = $<HTMLInputElement>("toggle-shortest");
const showTrails = $<HTMLInputElement>("toggle-trails");

function stateFrame(s: TurnLog["state"]): Frame {
  return { t: 0, ship: s.ship, load: s.load, heading_deg: s.heading_deg };
}

function startFrame(map: MapView): Frame {
  const cs = map.cell_size;
  const ship = { x: (map.start.x + 0.5) * cs, y: (map.start.y + 0.5) * cs };
  return { t: 0, ship, load: { x: ship.x, y: ship.y + map.physics.rope_length }, heading_deg: 0 };
}

function newView(summary: RunSummary, map: MapView | null): RunView {
  return { summary, map, turns: [], played: 0, queue: [], clipTime: 0, idle: map ? startFrame(map) : null };
}

function clipOf(turn: TurnLog): Clip {
  return { frames: turn.frames, after: stateFrame(turn.next), turn };
}

function pushTurn(view: RunView, turn: TurnLog, animate: boolean): void {
  view.turns.push(turn);
  if (animate) {
    view.queue.push(clipOf(turn));
  } else {
    view.played = view.turns.length;
    view.idle = stateFrame(turn.next);
  }
}

function resetMap(view: RunView, map: MapView): void {
  view.map = map;
  view.turns = [];
  view.played = 0;
  view.queue = [];
  view.idle = startFrame(map);
}

// ---------------------------------------------------------------- live feed

function connect(): void {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/ws`);
  ws.onopen = () => {
    wsStatus = "live";
    renderStatus();
  };
  ws.onclose = () => {
    wsStatus = "disconnected, retrying";
    renderStatus();
    setTimeout(connect, 2000);
  };
  ws.onmessage = (msg) => {
    const ev = JSON.parse(msg.data as string) as WsEvent;
    handleEvent(ev);
  };
}

function handleEvent(ev: WsEvent): void {
  switch (ev.type) {
    case "snapshot": {
      const seen = new Set<string>();
      for (const r of ev.runs) {
        seen.add(r.id);
        const view = runs.get(r.id);
        if (view) view.summary = r;
        else runs.set(r.id, newView(r, null));
      }
      for (const id of [...runs.keys()]) if (!seen.has(id) && id !== "replay") runs.delete(id);
      // Views without geometry (server restart / late join) fetch it.
      for (const [id, view] of runs) if (!view.map && id !== "replay") void fetchLiveMap(id);
      if (!replay && (!selectedId || !runs.has(selectedId))) selectedId = ev.runs[0]?.id ?? null;
      break;
    }
    case "run_started": {
      runs.set(ev.run.id, newView(ev.run, ev.map));
      if (!replay && (!selectedId || !runs.has(selectedId))) selectedId = ev.run.id;
      break;
    }
    case "turn": {
      let view = runs.get(ev.run.id);
      if (!view) {
        view = newView(ev.run, null);
        runs.set(ev.run.id, view);
        void fetchLiveMap(ev.run.id);
      }
      view.summary = ev.run;
      pushTurn(view, ev.turn, ev.run.id === selectedId && !replay);
      break;
    }
    case "map_started": {
      const view = runs.get(ev.run.id);
      if (view) {
        view.summary = ev.run;
        // Let the running animation finish before switching the map.
        const sw = () => resetMap(view, ev.map);
        if (view.queue.length && ev.run.id === selectedId) pendingMapSwitch.set(ev.run.id, sw);
        else sw();
      }
      break;
    }
    case "map_finished": {
      const view = runs.get(ev.run.id);
      if (view) view.summary = ev.run;
      break;
    }
    case "run_finished":
    case "run_aborted": {
      const view = runs.get(ev.run.id);
      if (view) view.summary = ev.run;
      finishedRuns.add(ev.run.id);
      void loadRecent();
      break;
    }
  }
  renderSidebar();
}

const pendingMapSwitch = new Map<string, () => void>();
const finishedRuns = new Set<string>();

async function fetchLiveMap(id: string): Promise<void> {
  const view = runs.get(id);
  if (!view) return;
  try {
    const res = await fetch(`/api/runs/${id}`);
    if (!res.ok) return;
    const detail = (await res.json()) as RunDetail;
    const map = detail.maps[detail.run.map_index] ?? null;
    if (map && !view.map) {
      resetMap(view, map);
      for (const t of detail.turns.filter((t) => t.map_index === map.index)) pushTurn(view, t, false);
    }
  } catch {
    /* retried on the next snapshot */
  }
}

async function loadRecent(): Promise<void> {
  try {
    const res = await fetch("/api/runs?limit=40");
    if (res.ok) {
      recentRuns = ((await res.json()) as RunSummary[]).filter((r) => r.status !== "running");
      renderSidebar();
    }
  } catch {
    /* ignore */
  }
}

// ------------------------------------------------------------------ replay

async function openReplay(id: string): Promise<void> {
  const res = await fetch(`/api/runs/${id}`);
  if (!res.ok) {
    alert(`run ${id} not found`);
    return;
  }
  const detail = (await res.json()) as RunDetail;
  replay = { detail, mapIdx: 0, cursor: 0, playing: false };
  const firstMap = detail.turns.length ? detail.turns[0].map_index : 0;
  setReplayMap(firstMap);
  history.replaceState(null, "", `?replay=${id}`);
  renderSidebar();
}

function replayView(): RunView | null {
  if (!replay) return null;
  let view = runs.get("replay");
  if (!view) {
    view = newView(replay.detail.run, null);
    runs.set("replay", view);
  }
  return view;
}

function replayTurns(): TurnLog[] {
  if (!replay) return [];
  return replay.detail.turns.filter((t) => t.map_index === replay!.mapIdx);
}

function setReplayMap(idx: number): void {
  if (!replay) return;
  replay.mapIdx = idx;
  replay.cursor = 0;
  replay.playing = false;
  const view = replayView()!;
  view.summary = replay.detail.run;
  resetMap(view, replay.detail.maps[idx]);
  selectedId = "replay";
  renderSidebar();
}

function replaySeek(cursor: number): void {
  if (!replay) return;
  const view = replayView()!;
  const turns = replayTurns();
  replay.cursor = Math.max(0, Math.min(cursor, turns.length));
  view.turns = turns.slice(0, replay.cursor);
  view.played = view.turns.length;
  view.queue = [];
  view.idle = replay.cursor > 0 ? stateFrame(turns[replay.cursor - 1].next) : startFrame(view.map!);
}

function replayStep(): void {
  if (!replay) return;
  replay.playing = false;
  feedReplay();
}

function feedReplay(): void {
  if (!replay) return;
  const view = replayView()!;
  const turns = replayTurns();
  if (view.queue.length === 0 && replay.cursor < turns.length) {
    const t = turns[replay.cursor++];
    view.turns.push(t);
    view.queue.push(clipOf(t));
  } else if (replay.cursor >= turns.length && view.queue.length === 0) {
    replay.playing = false;
  }
}

function closeReplay(): void {
  replay = null;
  runs.delete("replay");
  selectedId = [...runs.keys()][0] ?? null;
  history.replaceState(null, "", location.pathname);
  renderSidebar();
}

// ----------------------------------------------------------------- render

function currentView(): RunView | null {
  if (replay) return replayView();
  return selectedId ? runs.get(selectedId) ?? null : null;
}

function trailsOf(view: RunView, current: Frame | null): Trail[] {
  const byAttempt = new Map<number, Trail>();
  const playedTurns = view.turns.slice(0, view.played);
  const currentAttempt = view.queue[0]?.turn.attempt ?? playedTurns[playedTurns.length - 1]?.attempt ?? 1;
  for (const t of playedTurns) {
    let trail = byAttempt.get(t.attempt);
    if (!trail) {
      trail = { points: [], failed: false };
      byAttempt.set(t.attempt, trail);
    }
    for (const f of t.frames) trail.points.push(f.ship);
    if (t.event === "wall" || t.event === "turn_limit") trail.failed = true;
  }
  const clip = view.queue[0];
  if (clip && current) {
    let trail = byAttempt.get(clip.turn.attempt);
    if (!trail) {
      trail = { points: [], failed: false };
      byAttempt.set(clip.turn.attempt, trail);
    }
    for (const f of clip.frames) {
      if (f.t <= current.t) trail.points.push(f.ship);
    }
    trail.points.push(current.ship);
  }
  const out = [...byAttempt.entries()].map(([attempt, trail]) => ({
    ...trail,
    failed: trail.failed || attempt < currentAttempt,
  }));
  return out;
}

function tick(now: number): void {
  const dt = Math.min(0.25, (now - lastTick) / 1000);
  lastTick = now;
  const view = currentView();
  if (replay?.playing) feedReplay();
  let frame: Frame | null = null;
  let thrust = 0;
  if (view && view.map) {
    if (view.queue.length) {
      const clip = view.queue[0];
      const backlog = view.queue.length;
      const rate = replay ? 1 : Math.min(8, 1 + Math.max(0, backlog - 2) * 0.5);
      if (view.clipTime < clip.frames[0].t) view.clipTime = clip.frames[0].t;
      view.clipTime += dt * rate;
      const last = clip.frames[clip.frames.length - 1];
      if (view.clipTime >= last.t) {
        frame = last;
        view.queue.shift();
        view.played = Math.min(view.turns.length, view.played + 1);
        view.idle = clip.after;
        view.clipTime = view.queue[0]?.frames[0]?.t ?? 0;
        if (clip.turn.event === "wall" || clip.turn.event === "turn_limit" || clip.turn.event === "goal") {
          // Hold the end state briefly so the crash or exit is visible.
          view.clipTime -= 0.6;
        }
        const sw = pendingMapSwitch.get(view.summary.id);
        if (sw && view.queue.length === 0) {
          pendingMapSwitch.delete(view.summary.id);
          sw();
        }
      } else {
        frame = frameAt(clip.frames, view.clipTime) ?? view.idle;
        thrust = clip.turn.command.thrust;
      }
    } else {
      frame = view.idle;
    }
    renderer.draw(view.map, frame, trailsOf(view, view.queue.length ? frame : null), {
      showShortest: showShortest.checked,
      showTrails: showTrails.checked,
      thrust,
    });
    renderTelemetry(view, frame);
  } else {
    const ctx = canvas.getContext("2d")!;
    ctx.fillStyle = "#0f1218";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    controlsKey = "";
    $("telemetry").innerHTML = `<p class="muted">No run selected. Start an agent with the MCP tool <code>start_run</code>, or pick a finished run on the left to replay it.</p>`;
    $("map-header").textContent = "";
  }
  requestAnimationFrame(tick);
}

function fmt(n: number, d = 2): string {
  return n.toFixed(d);
}

function renderStatus(): void {
  $("ws-status").textContent = wsStatus;
}

let controlsKey = "";
function renderTelemetry(view: RunView, frame: Frame | null): void {
  const s = view.summary;
  const mapIdx = view.map ? view.map.index : s.map_index;
  const shownTurn = view.queue[0]?.turn ?? view.turns[view.played - 1] ?? null;
  const st = shownTurn ? (view.queue.length ? shownTurn.state : shownTurn.next) : null;

  // Static part: header, controls and turn list. Re-rendered only when
  // something structural changes, so buttons stay clickable.
  const key = [replay ? "r" : "l", s.id, mapIdx, view.turns.length, view.played, replay?.playing, s.status, s.score, s.attempt, s.turn, shownTurn?.note].join("|");
  if (key !== controlsKey) {
    controlsKey = key;
    const scores = s.map_scores
      .map((v, i) => {
        const cls = i === mapIdx && s.status === "running" ? "pill active" : v === null ? "pill" : v > 0 ? "pill ok" : "pill fail";
        return `<span class="${cls}">M${i + 1}${v === null ? "" : `: ${v}`}</span>`;
      })
      .join(" ");
    $("map-header").innerHTML = `<strong>${esc(s.agent_name)}</strong> <span class="muted">(${esc(s.model)})</span> · set ${esc(s.set_name)} · ${scores} · <strong>${s.score} pts</strong>${s.status !== "running" ? ` · <span class="muted">${s.status}</span>` : ""}`;

    const header = replay
      ? `<div class="replay-bar">
          <span class="muted">Replay</span>
          <button id="rp-play">${replay.playing ? "Pause" : "Play"}</button>
          <button id="rp-step">Step</button>
          <button id="rp-restart">Restart</button>
          <select id="rp-map">${replay.detail.maps.map((m) => `<option value="${m.index}" ${m.index === replay!.mapIdx ? "selected" : ""}>Map ${m.index + 1} (${m.width}x${m.height})</option>`).join("")}</select>
          <span class="muted">turn ${view.played}/${replayTurns().length}</span>
          <button id="rp-close">Close</button>
        </div>`
      : `<div class="replay-bar">
          <span class="muted">Live · attempt ${s.attempt}/${view.map?.attempt_limit ?? "?"} · turn ${s.turn}/${view.map?.turn_limit ?? "?"}</span>
          ${s.status === "running" ? `<button id="abort" class="danger">Abort run</button>` : ""}
        </div>`;
    const kv = `<table class="kv">
        ${row("Ship position", '<span id="kv-pos"></span>')}
        ${row("Ship velocity", '<span id="kv-vel"></span>')}
        ${row("Heading", '<span id="kv-heading"></span>')}
        ${row("Rope angle", '<span id="kv-rope"></span>')}
        ${row("Rope ang. velocity", '<span id="kv-ropev"></span>')}
        ${row("Last command", shownTurn ? `rotate ${fmt(shownTurn.command.rotate_deg, 1)}°, thrust ${fmt(shownTurn.command.thrust)}` : '<span class="muted">—</span>')}
        ${row("Event", shownTurn ? `<span class="ev ${shownTurn.event}">${shownTurn.event}</span>` : '<span class="muted">—</span>')}
        ${row("Note", shownTurn?.note ? esc(shownTurn.note) : '<span class="muted">—</span>')}
      </table>`;
    const turnsHtml = view.turns
      .map((t, i) => {
        const cls = i < view.played ? "" : "pending";
        return `<li class="${cls} ${replay ? "clickable" : ""}" data-i="${i}"><span class="n">a${t.attempt} t${t.turn}</span> <span>${fmt(t.command.rotate_deg, 0)}° / ${fmt(t.command.thrust, 2)}</span> <span class="ev ${t.event}">${t.event}</span>${t.note ? `<div class="note">${esc(t.note)}</div>` : ""}</li>`;
      })
      .join("");
    const telemetry = $("telemetry");
    telemetry.innerHTML = `${header}${kv}<h3>Turns (this map)</h3><ol id="turn-list" class="turns">${turnsHtml}</ol>`;
    const list = $("turn-list");
    if (!replay) list.scrollTop = list.scrollHeight;
    if (replay) {
      $("rp-play").onclick = () => {
        replay!.playing = !replay!.playing;
      };
      $("rp-step").onclick = () => replayStep();
      $("rp-restart").onclick = () => replaySeek(0);
      $("rp-close").onclick = () => closeReplay();
      ($("rp-map") as HTMLSelectElement).onchange = (e) => setReplayMap(Number((e.target as HTMLSelectElement).value));
      for (const li of telemetry.querySelectorAll<HTMLLIElement>("li.clickable")) {
        li.onclick = () => replaySeek(Number(li.dataset.i) + 1);
      }
    } else {
      const abort = document.getElementById("abort");
      if (abort) {
        abort.onclick = async () => {
          if (!confirm(`Abort run of ${s.agent_name}? The run ends with its current points.`)) return;
          await fetch(`/api/runs/${s.id}/abort`, { method: "POST" });
        };
      }
    }
  }

  // Dynamic part: numbers that follow the animation.
  if (frame) {
    const vel = st?.ship_vel ?? { x: 0, y: 0 };
    const dx = frame.load.x - frame.ship.x;
    const dy = frame.load.y - frame.ship.y;
    $("kv-pos").textContent = `${fmt(frame.ship.x)}, ${fmt(frame.ship.y)}`;
    $("kv-vel").textContent = `${fmt(vel.x)}, ${fmt(vel.y)} u/s`;
    $("kv-heading").textContent = `${fmt(frame.heading_deg, 1)}°`;
    $("kv-rope").textContent = `${fmt((Math.atan2(dx, dy) * 180) / Math.PI, 1)}° ${st ? (st.rope_taut ? "taut" : "slack") : ""}`;
    $("kv-ropev").textContent = st ? `${fmt(st.rope_angular_velocity_deg, 1)}°/s` : "";
  }
}

function row(k: string, v: string): string {
  return `<tr><th>${k}</th><td>${v}</td></tr>`;
}

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

function renderSidebar(): void {
  const live = [...runs.values()].filter((v) => v.summary.id !== "replay" && v.summary.status === "running");
  live.sort((a, b) => a.summary.started_at.localeCompare(b.summary.started_at));
  const liveHtml = live.length
    ? live
        .map((v) => {
          const s = v.summary;
          const sel = !replay && s.id === selectedId ? "selected" : "";
          const maps = s.map_scores
            .map((sc, i) => `<span class="dot ${sc === null ? (i === s.map_index ? "active" : "") : sc > 0 ? "ok" : "fail"}" title="Map ${i + 1}${sc === null ? "" : `: ${sc}`}"></span>`)
            .join("");
          return `<li class="run ${sel}" data-id="${s.id}">
            <div class="name">${esc(s.agent_name)} <span class="muted">${esc(s.model)}</span></div>
            <div class="meta">set ${esc(s.set_name)} · map ${Math.min(s.map_index + 1, s.map_count)}/${s.map_count} · attempt ${s.attempt} · turn ${s.turn}</div>
            <div class="meta">${maps} <strong>${s.score} pts</strong></div>
          </li>`;
        })
        .join("")
    : `<li class="muted empty">No agent is flying right now.</li>`;
  $("live-list").innerHTML = liveHtml;
  for (const li of $("live-list").querySelectorAll<HTMLLIElement>("li.run")) {
    li.onclick = () => {
      if (replay) closeReplay();
      selectedId = li.dataset.id!;
      const v = runs.get(selectedId);
      if (v) {
        v.queue = [];
        v.played = v.turns.length;
        const last = v.turns[v.turns.length - 1];
        v.idle = last ? stateFrame(last.next) : v.map ? startFrame(v.map) : null;
      }
      renderSidebar();
    };
  }
  const recentHtml = recentRuns.length
    ? recentRuns
        .slice(0, 25)
        .map((s) => {
          const sel = replay && replay.detail.run.id === s.id ? "selected" : "";
          return `<li class="run ${sel}" data-id="${s.id}">
            <div class="name">${esc(s.agent_name)} <span class="muted">${esc(s.model)}</span></div>
            <div class="meta">set ${esc(s.set_name)} · ${s.status} · ${s.total_turns} turns · <strong>${s.score} pts</strong></div>
          </li>`;
        })
        .join("")
    : `<li class="muted empty">No finished runs yet.</li>`;
  $("recent-list").innerHTML = recentHtml;
  for (const li of $("recent-list").querySelectorAll<HTMLLIElement>("li.run")) {
    li.onclick = () => void openReplay(li.dataset.id!);
  }
}

// ------------------------------------------------------------------ start

connect();
void loadRecent();
const replayParam = new URLSearchParams(location.search).get("replay");
if (replayParam) void openReplay(replayParam);
requestAnimationFrame(tick);
