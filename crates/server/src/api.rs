//! REST API and WebSocket stream for the browser.

use crate::state::{AppState, MapView, RunStatus, RunSummary, TurnLog, WsEvent};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/sets", get(list_sets))
        .route("/api/sets/{name}", get(get_set))
        .route("/api/runs", get(list_runs))
        .route("/api/runs/{id}", get(get_run))
        .route("/api/runs/{id}/abort", post(abort_run))
        .route("/api/leaderboard", get(leaderboard))
        .route("/ws", get(ws_handler))
        .with_state(state)
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

fn internal(e: anyhow::Error) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

#[derive(Serialize)]
struct SetSummary {
    name: String,
    description: String,
    generator_version: u32,
    turn_limit: u32,
    attempt_limit: u32,
    maps: Vec<MapMeta>,
}

#[derive(Serialize)]
struct MapMeta {
    index: usize,
    seed: u64,
    width: usize,
    height: usize,
    loop_fraction: f64,
    class: String,
    difficulty: Option<f64>,
    pilot_turns: Option<u32>,
}

fn set_summary(set: &cold_trap_sim::set::MapSet) -> SetSummary {
    SetSummary {
        name: set.name.clone(),
        description: set.description.clone(),
        generator_version: set.generator_version,
        turn_limit: set.turn_limit,
        attempt_limit: set.attempt_limit,
        maps: set
            .maps
            .iter()
            .enumerate()
            .map(|(i, m)| MapMeta {
                index: i,
                seed: m.seed,
                width: m.maze.width,
                height: m.maze.height,
                loop_fraction: m.maze.loop_fraction,
                class: m.class.clone(),
                difficulty: m.difficulty.as_ref().map(|d| d.score),
                pilot_turns: m.pilot_turns,
            })
            .collect(),
    }
}

async fn list_sets(State(state): State<Arc<AppState>>) -> Json<Vec<SetSummary>> {
    let mut sets: Vec<SetSummary> = state.sets.values().map(|s| set_summary(s)).collect();
    sets.sort_by(|a, b| a.name.cmp(&b.name));
    Json(sets)
}

#[derive(Serialize)]
struct SetDetail {
    #[serde(flatten)]
    summary: SetSummary,
    views: Vec<MapView>,
}

async fn get_set(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<SetDetail>, ApiError> {
    let set = state
        .sets
        .get(&name)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("unknown set {name}")))?;
    Ok(Json(SetDetail {
        summary: set_summary(set),
        views: state.set_map_views(set),
    }))
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<usize>,
}

async fn list_runs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<RunSummary>>, ApiError> {
    let limit = q.limit.unwrap_or(100).min(1000);
    let live = state.live_summaries();
    let mut out = live.clone();
    for r in state.db.list_runs(limit).map_err(internal)? {
        if !live.iter().any(|l| l.id == r.id) {
            out.push(r);
        }
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    out.truncate(limit);
    Ok(Json(out))
}

#[derive(Serialize)]
struct RunDetail {
    run: RunSummary,
    maps: Vec<MapView>,
    turns: Vec<TurnLog>,
}

async fn get_run(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<RunDetail>, ApiError> {
    let uuid = Uuid::parse_str(&id)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid run id".into()))?;
    let run = match state.get_run(&id) {
        Ok(live) => live.lock().unwrap().summary(),
        Err(_) => state
            .db
            .get_run(&uuid)
            .map_err(internal)?
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "unknown run".into()))?,
    };
    let set = state.sets.get(&run.set_name).ok_or_else(|| {
        ApiError(
            StatusCode::NOT_FOUND,
            format!("set {} is no longer loaded", run.set_name),
        )
    })?;
    let maps = state.set_map_views(set);
    let turns = state.db.list_turns(&uuid).map_err(internal)?;
    Ok(Json(RunDetail { run, maps, turns }))
}

async fn abort_run(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<RunSummary>, ApiError> {
    state
        .abort(&id)
        .map(Json)
        .map_err(|e| ApiError(StatusCode::NOT_FOUND, e.to_string()))
}

#[derive(Serialize)]
struct LeaderboardEntry {
    set_name: String,
    model: String,
    /// Agents that ran this model on this set.
    agents: Vec<String>,
    runs: usize,
    /// Median score over all finished runs (the model's value).
    median_score: i64,
    /// Fewer than three runs: the median is provisional.
    provisional: bool,
    best_score: i64,
    best_run_id: Uuid,
    best_run_turns: u32,
    maps_completed_best: usize,
    latest: chrono::DateTime<chrono::Utc>,
    run_ids: Vec<Uuid>,
}

async fn leaderboard(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<LeaderboardEntry>>, ApiError> {
    let finished = state.db.finished_runs().map_err(internal)?;
    let mut groups: HashMap<(String, String), Vec<RunSummary>> = HashMap::new();
    for r in finished
        .into_iter()
        .filter(|r| r.status == RunStatus::Finished)
    {
        groups
            .entry((r.set_name.clone(), r.model.clone()))
            .or_default()
            .push(r);
    }
    let mut out: Vec<LeaderboardEntry> = groups
        .into_iter()
        .map(|((set_name, model), mut runs)| {
            runs.sort_by_key(|r| std::cmp::Reverse(r.score));
            let best = runs[0].clone();
            let mut scores: Vec<i64> = runs.iter().map(|r| r.score).collect();
            scores.sort();
            let n = scores.len();
            let median = if n % 2 == 1 {
                scores[n / 2]
            } else {
                (scores[n / 2 - 1] + scores[n / 2]) / 2
            };
            let mut agents: Vec<String> = runs.iter().map(|r| r.agent_name.clone()).collect();
            agents.sort();
            agents.dedup();
            LeaderboardEntry {
                set_name,
                model,
                agents,
                runs: n,
                median_score: median,
                provisional: n < 3,
                best_score: best.score,
                best_run_id: best.id,
                best_run_turns: best.total_turns,
                maps_completed_best: best
                    .map_results
                    .iter()
                    .flatten()
                    .filter(|m| m.completed)
                    .count(),
                latest: runs
                    .iter()
                    .map(|r| r.started_at)
                    .max()
                    .unwrap_or(best.started_at),
                run_ids: runs.iter().map(|r| r.id).collect(),
            }
        })
        .collect();
    out.sort_by(|a, b| {
        a.set_name
            .cmp(&b.set_name)
            .then(b.median_score.cmp(&a.median_score))
            .then(b.best_score.cmp(&a.best_score))
    });
    Ok(Json(out))
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sink, mut stream) = socket.split();
    let mut rx = state.events.subscribe();
    let snapshot = WsEvent::Snapshot {
        runs: state.live_summaries(),
    };
    if let Ok(text) = serde_json::to_string(&snapshot) {
        if sink.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }
    loop {
        tokio::select! {
            ev = rx.recv() => {
                match ev {
                    Ok(ev) => {
                        let Ok(text) = serde_json::to_string(&ev) else { continue };
                        if sink.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // The browser fell behind; resend a snapshot so its list stays right.
                        let snapshot = WsEvent::Snapshot { runs: state.live_summaries() };
                        if let Ok(text) = serde_json::to_string(&snapshot) {
                            if sink.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    // Spectators have nothing to say; ignore anything else.
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}
