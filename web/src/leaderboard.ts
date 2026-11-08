// Leaderboard page: median per model and set, best runs, all finished runs.

import type { LeaderboardEntry, RunSummary } from "./types.js";

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

async function load(): Promise<void> {
  const [lb, runs] = await Promise.all([
    fetch("/api/leaderboard").then((r) => r.json() as Promise<LeaderboardEntry[]>),
    fetch("/api/runs?limit=200").then((r) => r.json() as Promise<RunSummary[]>),
  ]);
  const sets = [...new Set(lb.map((e) => e.set_name))].sort();
  const board = document.getElementById("board")!;
  if (!sets.length) {
    board.innerHTML = `<p class="muted">No finished runs yet. Finished runs appear here grouped by set and model; a model's value is the median of its runs.</p>`;
  } else {
    board.innerHTML = sets
      .map((set) => {
        const rows = lb
          .filter((e) => e.set_name === set)
          .map(
            (e, i) => `<tr>
              <td>${i + 1}</td>
              <td><strong>${esc(e.model)}</strong><div class="muted small">${e.agents.map(esc).join(", ")}</div></td>
              <td class="num">${e.median_score}${e.provisional ? ' <span class="muted" title="fewer than three runs">*</span>' : ""}</td>
              <td class="num"><a href="/?replay=${e.best_run_id}">${e.best_score}</a></td>
              <td class="num">${e.maps_completed_best}</td>
              <td class="num">${e.best_run_turns}</td>
              <td class="num">${e.runs}</td>
            </tr>`,
          )
          .join("");
        return `<h2>Set ${esc(set)}</h2>
          <table class="board">
            <thead><tr><th>#</th><th>Model</th><th class="num">Median</th><th class="num">Best run</th><th class="num">Maps (best)</th><th class="num">Turns (best)</th><th class="num">Runs</th></tr></thead>
            <tbody>${rows}</tbody>
          </table>
          <p class="muted small">* fewer than three runs: provisional. Click a best score to replay it.</p>`;
      })
      .join("");
  }
  const list = document.getElementById("runs")!;
  const finished = runs.filter((r) => r.status !== "running");
  list.innerHTML = finished.length
    ? `<table class="board"><thead><tr><th>Started</th><th>Agent</th><th>Model</th><th>Set</th><th>Status</th><th class="num">Turns</th><th class="num">Score</th><th>Maps</th><th></th></tr></thead><tbody>${finished
        .map(
          (r) => `<tr>
            <td>${new Date(r.started_at).toLocaleString()}</td>
            <td>${esc(r.agent_name)}</td><td>${esc(r.model)}</td><td>${esc(r.set_name)}</td><td>${r.status}</td>
            <td class="num">${r.total_turns}</td><td class="num"><strong>${r.score}</strong></td>
            <td>${r.map_scores.map((s) => (s === null ? "–" : s)).join(" / ")}</td>
            <td><a href="/?replay=${r.id}">replay</a></td>
          </tr>`,
        )
        .join("")}</tbody></table>`
    : `<p class="muted">No runs recorded.</p>`;
}

void load();
