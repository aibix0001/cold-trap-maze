//! The MCP surface: four tools over streamable HTTP.

use crate::state::{AppState, MapView, RunState, TurnState};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StartRunArgs {
    /// Name of the agent, shown in the browser and the leaderboard.
    pub agent_name: String,
    /// Name of the benchmark set to fly (e.g. "default").
    pub set_name: String,
    /// Model that drives the agent, e.g. "claude-opus-5-5". Used to group
    /// runs in the leaderboard. Optional.
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunArgs {
    /// The run id returned by start_run.
    pub run_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ActArgs {
    /// The run id returned by start_run.
    pub run_id: String,
    /// Rotation for this turn in degrees, clockwise positive. Clamped to
    /// the map's limit (45 by default). Applied gradually over the turn.
    pub rotate_deg: f64,
    /// Thrust fraction between 0 and 1, applied along the heading for the
    /// whole turn.
    pub thrust: f64,
    /// Free-text note about your plan. Logged and shown to spectators,
    /// never scored.
    #[serde(default)]
    pub note: Option<String>,
}

/// Structured companion of the text result, for programmatic clients.
#[derive(Debug, Serialize)]
pub struct Structured<'a> {
    pub run_id: String,
    pub status: &'a str,
    pub map_index: usize,
    pub map_count: usize,
    pub score: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map: Option<MapView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<TurnState>,
    pub turn: u32,
    pub attempt: u32,
}

#[derive(Clone)]
pub struct ColdTrapMcp {
    state: Arc<AppState>,
    tool_router: ToolRouter<Self>,
}

pub const INSTRUCTIONS: &str = "Cold Trap Maze: a turn-based thrust labyrinth. You fly a ship with a load hanging from a rope out of a maze. \
Call start_run(agent_name, set_name) to get a run_id and the first observation, then act(run_id, rotate_deg, thrust, note) once per turn. \
Each turn simulates 0.5 s: your rotation (max +/-45 deg, clockwise positive, 0 = up) is applied gradually and the thrust (0..1) acts along the heading. \
Gravity pulls down (+y). Thrust only pushes forward, so braking means turning and thrusting. Touching a wall with ship, rope or load ends the attempt and resets you to the start; \
after the attempt limit the map scores 0. Points per map: 1000 for completing, -150 per extra attempt, -1 per turn, +250 if you only flew through cells on a shortest path. \
observe(run_id) repeats the current observation, give_up(run_id) skips the current map. The map is given as ASCII: '+--+' are walls, spaces are openings, S start, E exit, @ your ship.";

fn text_result(text: String, structured: serde_json::Value) -> CallToolResult {
    let mut r = CallToolResult::success(vec![ContentBlock::text(text)]);
    r.structured_content = Some(structured);
    r
}

fn structured_of(run: &RunState, event: Option<&str>, with_map: bool) -> serde_json::Value {
    let (turn, attempt) = run
        .game
        .as_ref()
        .map(|g| (g.turn(), g.attempt()))
        .unwrap_or((0, 0));
    let state = run
        .game
        .as_ref()
        .map(|g| TurnState::from_physics(g.state(), &g.config().physics));
    let s = Structured {
        run_id: run.id.to_string(),
        status: run.status.as_str(),
        map_index: run.map_index,
        map_count: run.set.maps.len(),
        score: run.score(),
        event,
        map: if with_map { run.map_view() } else { None },
        state,
        turn,
        attempt,
    };
    serde_json::to_value(s).unwrap_or(json!({}))
}

/// Structured content for a run that has left memory (finished/aborted).
fn structured_final(summary: &crate::state::RunSummary, event: Option<&str>) -> serde_json::Value {
    json!({
        "run_id": summary.id.to_string(),
        "status": summary.status.as_str(),
        "map_index": summary.map_index,
        "map_count": summary.map_count,
        "score": summary.score,
        "map_scores": summary.map_scores,
        "event": event,
        "turn": 0,
        "attempt": 0,
        "finished": true,
    })
}

fn invalid(e: anyhow::Error) -> ErrorData {
    ErrorData::invalid_params(e.to_string(), None)
}

#[tool_router(router = tool_router)]
impl ColdTrapMcp {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Start a benchmark run for an agent on a set. Returns the run_id and the first observation of map 1."
    )]
    async fn start_run(
        &self,
        Parameters(args): Parameters<StartRunArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let model = args.model.unwrap_or_else(|| "unknown".to_string());
        let (id, text) = self
            .state
            .start_run(&args.agent_name, &args.set_name, &model)
            .map_err(invalid)?;
        let run = self.state.get_run(&id.to_string()).map_err(invalid)?;
        let structured = structured_of(&run.lock().unwrap(), None, true);
        Ok(text_result(format!("run_id: {id}\n{text}"), structured))
    }

    #[tool(description = "Return the current observation of a run without spending a turn.")]
    async fn observe(
        &self,
        Parameters(args): Parameters<RunArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let text = self.state.observe(&args.run_id).map_err(invalid)?;
        let run = self.state.get_run(&args.run_id).map_err(invalid)?;
        let structured = structured_of(&run.lock().unwrap(), None, true);
        Ok(text_result(text, structured))
    }

    #[tool(
        description = "Fly one turn: rotate by rotate_deg (clockwise positive, max +/-45) and apply thrust (0..1) for 0.5 s. Returns the event (ok, wall, goal, turn_limit) and the new observation."
    )]
    async fn act(
        &self,
        Parameters(args): Parameters<ActArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let out = self
            .state
            .act(&args.run_id, args.rotate_deg, args.thrust, args.note)
            .map_err(invalid)?;
        let event = out.record.event.name();
        let structured = if out.run_finished {
            structured_final(&out.summary, Some(event))
        } else {
            match self.state.get_run(&args.run_id) {
                Ok(run) => structured_of(&run.lock().unwrap(), Some(event), out.new_map.is_some()),
                Err(_) => structured_final(&out.summary, Some(event)),
            }
        };
        Ok(text_result(
            format!("event: {event}\n{}", out.text),
            structured,
        ))
    }

    #[tool(
        description = "Give up the current map (it scores 0) and move on to the next map, or end the run after the last map."
    )]
    async fn give_up(
        &self,
        Parameters(args): Parameters<RunArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (text, _new_map, finished, summary) =
            self.state.give_up(&args.run_id).map_err(invalid)?;
        let structured = if finished {
            structured_final(&summary, Some("given_up"))
        } else {
            let run = self.state.get_run(&args.run_id).map_err(invalid)?;
            let guard = run.lock().unwrap();
            structured_of(&guard, Some("given_up"), true)
        };
        Ok(text_result(text, structured))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ColdTrapMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "cold-trap-maze",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }
}
