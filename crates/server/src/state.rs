//! In-memory run engine: live runs, scoring, and the event stream that
//! feeds the browser.

use crate::db::Db;
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use cold_trap_sim::game::{Command, Event, Game, GameConfig, TurnRecord};
use cold_trap_sim::maze::{Cell, Maze, Side};
use cold_trap_sim::physics::{Frame, PhysicsParams, PhysicsState};
use cold_trap_sim::score::{score_map, MapResult};
use cold_trap_sim::set::MapSet;
use cold_trap_sim::vec2::Vec2;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Shared application state.
pub struct AppState {
    pub sets: HashMap<String, Arc<MapSet>>,
    pub runs: RwLock<HashMap<Uuid, Arc<Mutex<RunState>>>>,
    pub db: Db,
    pub events: broadcast::Sender<WsEvent>,
}

/// Status of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Finished,
    Aborted,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Finished => "finished",
            RunStatus::Aborted => "aborted",
        }
    }
    pub fn parse(s: &str) -> RunStatus {
        match s {
            "finished" => RunStatus::Finished,
            "aborted" => RunStatus::Aborted,
            _ => RunStatus::Running,
        }
    }
}

/// Geometry of a map as the browser needs it to draw.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapView {
    pub index: usize,
    pub seed: u64,
    pub width: usize,
    pub height: usize,
    pub cell_size: f64,
    pub h_walls: Vec<bool>,
    pub v_walls: Vec<bool>,
    pub start: Cell,
    pub exit: Cell,
    pub exit_side: Side,
    /// Cells lying on any shortest path.
    pub shortest_cells: Vec<Cell>,
    /// One canonical shortest path.
    pub path: Vec<Cell>,
    pub shortest_length: usize,
    pub physics: PhysicsParams,
    pub turn_limit: u32,
    pub attempt_limit: u32,
}

impl MapView {
    pub fn new(index: usize, seed: u64, maze: &Maze, config: &GameConfig) -> Self {
        let sp = maze.shortest_paths();
        let shortest_cells = (0..maze.width * maze.height)
            .filter(|i| sp.on_shortest[*i])
            .map(|i| Cell::new(i % maze.width, i / maze.width))
            .collect();
        MapView {
            index,
            seed,
            width: maze.width,
            height: maze.height,
            cell_size: maze.cell_size(),
            h_walls: maze.h_walls.clone(),
            v_walls: maze.v_walls.clone(),
            start: maze.start,
            exit: maze.exit,
            exit_side: maze.exit_side,
            shortest_cells,
            path: sp.path.clone(),
            shortest_length: sp.length,
            physics: config.physics.clone(),
            turn_limit: config.turn_limit,
            attempt_limit: config.attempt_limit,
        }
    }
}

/// Compact dynamic state stored per turn and streamed to the browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnState {
    pub ship: Vec2,
    pub ship_vel: Vec2,
    pub heading_deg: f64,
    pub load: Vec2,
    pub load_vel: Vec2,
    pub rope_angle_deg: f64,
    pub rope_angular_velocity_deg: f64,
    pub rope_taut: bool,
}

impl TurnState {
    pub fn from_physics(s: &PhysicsState, p: &PhysicsParams) -> Self {
        TurnState {
            ship: s.ship.pos,
            ship_vel: s.ship.vel,
            heading_deg: s.heading_deg,
            load: s.load.pos,
            load_vel: s.load.vel,
            rope_angle_deg: s.rope_angle_deg(),
            rope_angular_velocity_deg: s.rope_angular_velocity_deg(),
            rope_taut: s.rope_taut(p),
        }
    }
}

/// One stored turn (also the replay unit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnLog {
    pub map_index: usize,
    pub attempt: u32,
    pub turn: u32,
    pub command: Command,
    pub note: Option<String>,
    pub event: Event,
    pub frames: Vec<Frame>,
    /// State at the end of the turn (before a reset, if any).
    pub state: TurnState,
    /// State the agent sees afterwards (after a reset or on the next map
    /// the start state), for the telemetry panel.
    pub next: TurnState,
    pub created_at: DateTime<Utc>,
}

/// Summary of a run for lists and the leaderboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: Uuid,
    pub agent_name: String,
    pub model: String,
    pub set_name: String,
    pub status: RunStatus,
    /// 0-based index of the current map (or map_count when finished).
    pub map_index: usize,
    pub map_count: usize,
    pub attempt: u32,
    pub turn: u32,
    pub total_turns: u32,
    pub score: i64,
    /// Points per finished map (None while a map is still being played).
    pub map_scores: Vec<Option<i64>>,
    pub map_results: Vec<Option<MapResult>>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub last_note: Option<String>,
}

/// Events pushed to browsers over the WebSocket.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsEvent {
    Snapshot {
        runs: Vec<RunSummary>,
    },
    RunStarted {
        run: RunSummary,
        map: MapView,
    },
    Turn {
        run: RunSummary,
        turn: TurnLog,
    },
    MapStarted {
        run: RunSummary,
        map: MapView,
    },
    MapFinished {
        run: RunSummary,
        map_index: usize,
        result: MapResult,
        points: i64,
    },
    RunFinished {
        run: RunSummary,
    },
    RunAborted {
        run: RunSummary,
    },
}

/// A live run.
pub struct RunState {
    pub id: Uuid,
    pub agent_name: String,
    pub model: String,
    pub set: Arc<MapSet>,
    pub map_index: usize,
    pub game: Option<Game>,
    pub map_results: Vec<Option<MapResult>>,
    pub map_scores: Vec<Option<i64>>,
    pub status: RunStatus,
    pub total_turns: u32,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub last_note: Option<String>,
}

impl RunState {
    fn game_config(set: &MapSet) -> GameConfig {
        GameConfig {
            physics: set.physics.clone(),
            turn_limit: set.turn_limit,
            attempt_limit: set.attempt_limit,
        }
    }

    pub fn new(agent_name: String, model: String, set: Arc<MapSet>) -> Self {
        let n = set.maps.len();
        let game = set
            .maps
            .first()
            .map(|m| Game::new(m.generate(), Self::game_config(&set)));
        RunState {
            id: Uuid::new_v4(),
            agent_name,
            model,
            set,
            map_index: 0,
            game,
            map_results: vec![None; n],
            map_scores: vec![None; n],
            status: RunStatus::Running,
            total_turns: 0,
            started_at: Utc::now(),
            finished_at: None,
            last_note: None,
        }
    }

    pub fn score(&self) -> i64 {
        self.map_scores.iter().flatten().sum()
    }

    pub fn summary(&self) -> RunSummary {
        let (attempt, turn) = self
            .game
            .as_ref()
            .map(|g| (g.attempt(), g.turn()))
            .unwrap_or((0, 0));
        RunSummary {
            id: self.id,
            agent_name: self.agent_name.clone(),
            model: self.model.clone(),
            set_name: self.set.name.clone(),
            status: self.status,
            map_index: self.map_index,
            map_count: self.set.maps.len(),
            attempt,
            turn,
            total_turns: self.total_turns,
            score: self.score(),
            map_scores: self.map_scores.clone(),
            map_results: self.map_results.clone(),
            started_at: self.started_at,
            finished_at: self.finished_at,
            last_note: self.last_note.clone(),
        }
    }

    pub fn map_view(&self) -> Option<MapView> {
        let game = self.game.as_ref()?;
        let spec = self.set.maps.get(self.map_index)?;
        Some(MapView::new(
            self.map_index,
            spec.seed,
            game.maze(),
            game.config(),
        ))
    }

    /// Header line for the observation text.
    pub fn header(&self) -> String {
        format!(
            "Run {} | agent {} | set {} | map {} of {} | score so far {}",
            self.id,
            self.agent_name,
            self.set.name,
            self.map_index + 1,
            self.set.maps.len(),
            self.score()
        )
    }

    /// Current observation text, or a closing message when finished.
    pub fn observation_text(&self) -> String {
        match (&self.status, &self.game) {
            (RunStatus::Running, Some(game)) => game.observation().to_text(&self.header()),
            _ => self.final_text(),
        }
    }

    pub fn final_text(&self) -> String {
        let per_map: Vec<String> = self
            .map_scores
            .iter()
            .enumerate()
            .map(|(i, s)| {
                format!(
                    "map {}: {}",
                    i + 1,
                    s.map(|v| v.to_string()).unwrap_or_else(|| "-".into())
                )
            })
            .collect();
        format!(
            "Run {} is {}. Total score {} ({}).",
            self.id,
            self.status.as_str(),
            self.score(),
            per_map.join(", ")
        )
    }

    /// Finish the current map and advance. Returns the finished map's
    /// result and points.
    fn finish_map(&mut self) -> (MapResult, i64) {
        let game = self.game.as_ref().expect("game present");
        let result = game.result().expect("map finished");
        let points = score_map(&result, &self.set.scoring);
        let idx = self.map_index;
        self.map_results[idx] = Some(result);
        self.map_scores[idx] = Some(points);
        self.map_index += 1;
        if self.map_index < self.set.maps.len() {
            let spec = &self.set.maps[self.map_index];
            self.game = Some(Game::new(spec.generate(), Self::game_config(&self.set)));
        } else {
            self.game = None;
            self.status = RunStatus::Finished;
            self.finished_at = Some(Utc::now());
        }
        (result, points)
    }
}

/// What happened as a consequence of an `act` call.
pub struct ActOutcome {
    pub record: TurnRecord,
    #[allow(dead_code)]
    pub map_finished: Option<(usize, MapResult, i64)>,
    pub new_map: Option<MapView>,
    pub run_finished: bool,
    pub text: String,
    /// Run summary right after the turn (also valid once the run is gone).
    pub summary: RunSummary,
}

impl AppState {
    pub fn new(sets: HashMap<String, Arc<MapSet>>, db: Db) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        Arc::new(AppState {
            sets,
            runs: RwLock::new(HashMap::new()),
            db,
            events,
        })
    }

    fn emit(&self, ev: WsEvent) {
        let _ = self.events.send(ev);
    }

    pub fn get_run(&self, id: &str) -> Result<Arc<Mutex<RunState>>> {
        let uuid = Uuid::parse_str(id).map_err(|_| anyhow!("invalid run_id '{id}'"))?;
        self.runs
            .read()
            .unwrap()
            .get(&uuid)
            .cloned()
            .ok_or_else(|| anyhow!("unknown or finished run '{id}'"))
    }

    /// Summaries of all runs kept in memory, newest first.
    pub fn live_summaries(&self) -> Vec<RunSummary> {
        let mut v: Vec<RunSummary> = self
            .runs
            .read()
            .unwrap()
            .values()
            .map(|r| r.lock().unwrap().summary())
            .collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.started_at));
        v
    }

    /// Start a run. Returns the run id and the first observation text.
    pub fn start_run(
        &self,
        agent_name: &str,
        set_name: &str,
        model: &str,
    ) -> Result<(Uuid, String)> {
        let set = self.sets.get(set_name).cloned().ok_or_else(|| {
            anyhow!(
                "unknown set '{set_name}'. Available sets: {}",
                self.set_names().join(", ")
            )
        })?;
        if set.maps.is_empty() {
            return Err(anyhow!("set '{set_name}' has no maps"));
        }
        let agent_name = agent_name.trim();
        if agent_name.is_empty() {
            return Err(anyhow!("agent_name must not be empty"));
        }
        let run = RunState::new(agent_name.to_string(), model.trim().to_string(), set);
        let id = run.id;
        let summary = run.summary();
        let map = run.map_view().expect("first map");
        let text = run.observation_text();
        self.db.insert_run(&summary)?;
        self.runs
            .write()
            .unwrap()
            .insert(id, Arc::new(Mutex::new(run)));
        self.emit(WsEvent::RunStarted { run: summary, map });
        Ok((id, text))
    }

    pub fn set_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.sets.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn observe(&self, run_id: &str) -> Result<String> {
        let run = self.get_run(run_id)?;
        let run = run.lock().unwrap();
        Ok(run.observation_text())
    }

    /// Execute one turn.
    pub fn act(
        &self,
        run_id: &str,
        rotate_deg: f64,
        thrust: f64,
        note: Option<String>,
    ) -> Result<ActOutcome> {
        let run_arc = self.get_run(run_id)?;
        let mut run = run_arc.lock().unwrap();
        if run.status != RunStatus::Running {
            return Err(anyhow!("{}", run.final_text()));
        }
        if !rotate_deg.is_finite() || !thrust.is_finite() {
            return Err(anyhow!("rotate_deg and thrust must be finite numbers"));
        }
        let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        run.last_note = note.clone();
        let map_index = run.map_index;
        run.total_turns += 1;
        let game = run.game.as_mut().expect("running run has a game");
        let max_rot = game.config().physics.max_rotate_deg;
        let cmd = Command {
            rotate_deg: rotate_deg.clamp(-max_rot, max_rot),
            thrust: thrust.clamp(0.0, 1.0),
        };
        let record = game.act(cmd);
        let state = TurnState::from_physics(&record.state_after, &game.config().physics);

        let mut text = String::new();
        let mut map_finished = None;
        let mut new_map = None;
        let mut run_finished = false;
        match record.event {
            Event::Ok => {}
            Event::Wall => {
                text.push_str(&format!(
                    "WALL: ship, rope or load touched a wall after {:.2} s of turn {} (attempt {}). ",
                    record.frames.last().map(|f| f.t).unwrap_or(0.0) - (record.turn - 1) as f64 * game.config().physics.turn_seconds,
                    record.turn,
                    record.attempt
                ));
            }
            Event::TurnLimit => {
                text.push_str(&format!(
                    "TURN LIMIT: attempt {} used all {} turns. ",
                    record.attempt,
                    game.config().turn_limit
                ));
            }
            Event::Goal => {
                text.push_str(&format!(
                    "GOAL: exit passed in attempt {} after {} turns. ",
                    record.attempt, record.turn
                ));
            }
        }
        if game.is_finished() {
            let (result, points) = run.finish_map();
            if !result.completed {
                text.push_str(&format!(
                    "Map {} failed after {} attempts: 0 points. ",
                    map_index + 1,
                    result.attempts
                ));
            } else {
                text.push_str(&format!(
                    "Map {} completed: {} points ({} attempt(s), {} turns{}). ",
                    map_index + 1,
                    points,
                    result.attempts,
                    result.turns,
                    if result.shortest_path {
                        ", shortest path bonus"
                    } else {
                        ""
                    }
                ));
            }
            map_finished = Some((map_index, result, points));
            if run.status == RunStatus::Running {
                new_map = run.map_view();
                text.push_str(&format!(
                    "Starting map {} of {}.\n",
                    run.map_index + 1,
                    run.set.maps.len()
                ));
            } else {
                run_finished = true;
            }
        } else if record.event != Event::Ok {
            text.push_str(&format!(
                "Reset to the start for attempt {} of {}.\n",
                game.attempt(),
                game.config().attempt_limit
            ));
        }
        if run.status == RunStatus::Running {
            text.push_str(&run.observation_text());
        } else {
            text.push_str(&run.final_text());
        }

        let next = run
            .game
            .as_ref()
            .map(|g| TurnState::from_physics(g.state(), &g.config().physics))
            .unwrap_or_else(|| state.clone());
        let log = TurnLog {
            map_index,
            attempt: record.attempt,
            turn: record.turn,
            command: cmd,
            note,
            event: record.event,
            frames: record.frames.clone(),
            state,
            next,
            created_at: Utc::now(),
        };
        let summary = run.summary();
        self.db.insert_turn(&run.id, &log)?;
        self.db.update_run(&summary)?;
        drop(run);

        self.emit(WsEvent::Turn {
            run: summary.clone(),
            turn: log.clone(),
        });
        if let Some((idx, result, points)) = &map_finished {
            self.emit(WsEvent::MapFinished {
                run: summary.clone(),
                map_index: *idx,
                result: *result,
                points: *points,
            });
        }
        if let Some(map) = &new_map {
            self.emit(WsEvent::MapStarted {
                run: summary.clone(),
                map: map.clone(),
            });
        }
        if run_finished {
            self.emit(WsEvent::RunFinished {
                run: summary.clone(),
            });
            self.runs.write().unwrap().remove(&summary.id);
        }
        Ok(ActOutcome {
            record,
            map_finished,
            new_map,
            run_finished,
            text,
            summary,
        })
    }

    /// Give up the current map (0 points) and move on. Returns the text,
    /// the new map (if any), whether the run is finished, and the summary.
    pub fn give_up(&self, run_id: &str) -> Result<(String, Option<MapView>, bool, RunSummary)> {
        let run_arc = self.get_run(run_id)?;
        let mut run = run_arc.lock().unwrap();
        if run.status != RunStatus::Running {
            return Err(anyhow!("{}", run.final_text()));
        }
        let map_index = run.map_index;
        run.game.as_mut().expect("game").give_up();
        let (result, points) = run.finish_map();
        let summary = run.summary();
        let new_map = run.map_view();
        let finished = run.status != RunStatus::Running;
        let mut text = format!("Map {} given up: {} points. ", map_index + 1, points);
        if finished {
            text.push_str(&run.final_text());
        } else {
            text.push_str(&format!(
                "Starting map {} of {}.\n",
                run.map_index + 1,
                run.set.maps.len()
            ));
            text.push_str(&run.observation_text());
        }
        self.db.update_run(&summary)?;
        drop(run);
        self.emit(WsEvent::MapFinished {
            run: summary.clone(),
            map_index,
            result,
            points,
        });
        if let Some(map) = &new_map {
            self.emit(WsEvent::MapStarted {
                run: summary.clone(),
                map: map.clone(),
            });
        }
        if finished {
            self.emit(WsEvent::RunFinished {
                run: summary.clone(),
            });
            self.runs.write().unwrap().remove(&summary.id);
        }
        Ok((text, new_map, finished, summary))
    }

    /// Operator abort: the run ends immediately with the points earned so far.
    pub fn abort(&self, run_id: &str) -> Result<RunSummary> {
        let run_arc = self.get_run(run_id)?;
        let mut run = run_arc.lock().unwrap();
        run.status = RunStatus::Aborted;
        run.finished_at = Some(Utc::now());
        run.game = None;
        let summary = run.summary();
        self.db.update_run(&summary)?;
        drop(run);
        self.emit(WsEvent::RunAborted {
            run: summary.clone(),
        });
        self.runs.write().unwrap().remove(&summary.id);
        Ok(summary)
    }

    /// Map views of every map in a set (for replays).
    pub fn set_map_views(&self, set: &MapSet) -> Vec<MapView> {
        let config = RunState::game_config(set);
        set.maps
            .iter()
            .enumerate()
            .map(|(i, spec)| MapView::new(i, spec.seed, &spec.generate(), &config))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cold_trap_pilot::{Pilot, PilotParams};
    use cold_trap_sim::maze::MazeParams;
    use cold_trap_sim::set::MapSpec;

    fn test_set() -> Arc<MapSet> {
        Arc::new(MapSet {
            name: "mini".into(),
            generator_version: cold_trap_sim::GENERATOR_VERSION,
            description: String::new(),
            physics: PhysicsParams::default(),
            scoring: Default::default(),
            turn_limit: 300,
            attempt_limit: 5,
            maps: vec![
                MapSpec {
                    seed: 23,
                    maze: MazeParams::new(8, 8, 0.0),
                    class: "easy".into(),
                    difficulty: None,
                    pilot_turns: None,
                },
                MapSpec {
                    seed: 30,
                    maze: MazeParams::new(10, 10, 0.0),
                    class: "easy".into(),
                    difficulty: None,
                    pilot_turns: None,
                },
            ],
        })
    }

    fn app() -> Arc<AppState> {
        let mut sets = HashMap::new();
        sets.insert("mini".to_string(), test_set());
        AppState::new(sets, Db::open_in_memory().unwrap())
    }

    #[test]
    fn full_run_with_pilot_is_scored_and_stored() {
        let app = app();
        let mut rx = app.events.subscribe();
        let (id, text) = app.start_run("tester", "mini", "unit-test").unwrap();
        assert!(text.contains("map 1 of 2"));
        let id = id.to_string();
        let mut finished = false;
        let mut map_finishes = 0;
        let mut pilot: Option<Pilot> = None;
        for _ in 0..2000 {
            let (cmd, map_index) = {
                let run = app.get_run(&id).unwrap();
                let run = run.lock().unwrap();
                let game = run.game.as_ref().unwrap();
                let p = pilot.get_or_insert_with(|| {
                    Pilot::for_maze(game.maze(), game.shortest(), PilotParams::default())
                });
                (
                    p.command(game.state(), &game.config().physics),
                    run.map_index,
                )
            };
            let out = app
                .act(
                    &id,
                    cmd.rotate_deg,
                    cmd.thrust,
                    Some(format!("map {map_index}")),
                )
                .unwrap();
            if matches!(out.record.event, Event::Wall | Event::TurnLimit) {
                pilot.as_mut().unwrap().reset();
            }
            if out.map_finished.is_some() {
                map_finishes += 1;
                pilot = None;
            }
            if out.run_finished {
                finished = true;
                break;
            }
        }
        assert!(finished, "run did not finish");
        assert_eq!(map_finishes, 2);
        assert!(app.get_run(&id).is_err(), "finished runs leave memory");
        let uuid = Uuid::parse_str(&id).unwrap();
        let stored = app.db.get_run(&uuid).unwrap().expect("stored");
        assert_eq!(stored.status, RunStatus::Finished);
        assert!(stored.score > 1500, "score {}", stored.score);
        let turns = app.db.list_turns(&uuid).unwrap();
        assert_eq!(turns.len() as u32, stored.total_turns);
        assert!(turns.iter().all(|t| t
            .note
            .as_deref()
            .map(|n| n.starts_with("map "))
            .unwrap_or(false)));
        // Events were broadcast: started, turns, map finished, run finished.
        let mut kinds = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            kinds.push(match ev {
                WsEvent::RunStarted { .. } => "started",
                WsEvent::Turn { .. } => "turn",
                WsEvent::MapStarted { .. } => "map_started",
                WsEvent::MapFinished { .. } => "map_finished",
                WsEvent::RunFinished { .. } => "run_finished",
                _ => "other",
            });
        }
        assert_eq!(kinds.first(), Some(&"started"));
        assert_eq!(kinds.last(), Some(&"run_finished"));
        assert_eq!(kinds.iter().filter(|k| **k == "map_finished").count(), 2);
        assert_eq!(kinds.iter().filter(|k| **k == "map_started").count(), 1);
    }

    #[test]
    fn give_up_and_abort() {
        let app = app();
        let (id, _) = app.start_run("quitter", "mini", "x").unwrap();
        let id = id.to_string();
        let (text, new_map, finished, _) = app.give_up(&id).unwrap();
        assert!(text.contains("given up"));
        assert!(new_map.is_some());
        assert!(!finished);
        let summary = app.abort(&id).unwrap();
        assert_eq!(summary.status, RunStatus::Aborted);
        assert_eq!(summary.map_scores, vec![Some(0), None]);
        assert!(app.get_run(&id).is_err());
        assert!(app.start_run("x", "nope", "").is_err());
        assert!(app.start_run("", "mini", "").is_err());
    }
}
