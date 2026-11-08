//! Reference pilot as an ordinary MCP client. It talks to the server over
//! streamable HTTP exactly like any agent app would, using only the four
//! tools, and flies the BFS path with the scripted controller.

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use cold_trap_pilot::{Pilot, PilotParams};
use cold_trap_sim::maze::{Maze, MazeParams};
use cold_trap_sim::physics::{Body, PhysicsParams, PhysicsState};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Parser, Debug)]
#[command(
    name = "cold-trap-pilot-client",
    about = "Reference pilot talking MCP to a Cold Trap Maze server"
)]
struct Args {
    /// MCP endpoint.
    #[arg(long, default_value = "http://127.0.0.1:8080/mcp")]
    url: String,
    #[arg(long, default_value = "default")]
    set: String,
    #[arg(long, default_value = "reference-pilot")]
    agent_name: String,
    #[arg(long, default_value = "reference-pilot")]
    model: String,
    /// Number of complete runs to fly in sequence.
    #[arg(long, default_value_t = 1)]
    runs: u32,
    /// Pause between turns in milliseconds (to watch in the browser).
    #[arg(long, default_value_t = 0)]
    delay_ms: u64,
    /// Print every observation text.
    #[arg(long)]
    verbose: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct MapJson {
    width: usize,
    height: usize,
    cell_size: f64,
    h_walls: Vec<bool>,
    v_walls: Vec<bool>,
    start: cold_trap_sim::maze::Cell,
    exit: cold_trap_sim::maze::Cell,
    exit_side: cold_trap_sim::maze::Side,
    seed: u64,
    physics: PhysicsParams,
}

#[derive(Debug, Deserialize)]
struct StateJson {
    ship: cold_trap_sim::vec2::Vec2,
    ship_vel: cold_trap_sim::vec2::Vec2,
    heading_deg: f64,
    load: cold_trap_sim::vec2::Vec2,
    load_vel: cold_trap_sim::vec2::Vec2,
}

#[derive(Debug, Deserialize)]
struct Structured {
    run_id: String,
    status: String,
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    map: Option<MapJson>,
    #[serde(default)]
    state: Option<StateJson>,
    #[serde(default)]
    score: i64,
    #[serde(default)]
    map_index: usize,
}

fn maze_from(m: &MapJson) -> Maze {
    Maze {
        seed: m.seed,
        params: MazeParams {
            width: m.width,
            height: m.height,
            loop_fraction: 0.0,
            cell_size: m.cell_size,
        },
        generator_version: cold_trap_sim::GENERATOR_VERSION,
        width: m.width,
        height: m.height,
        h_walls: m.h_walls.clone(),
        v_walls: m.v_walls.clone(),
        start: m.start,
        exit: m.exit,
        exit_side: m.exit_side,
    }
}

fn physics_state(s: &StateJson) -> PhysicsState {
    PhysicsState {
        ship: Body {
            pos: s.ship,
            vel: s.ship_vel,
        },
        load: Body {
            pos: s.load,
            vel: s.load_vel,
        },
        heading_deg: s.heading_deg,
        time: 0.0,
    }
}

fn parse(result: &CallToolResult) -> Result<(Structured, String)> {
    if result.is_error == Some(true) {
        let msg = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(anyhow!("tool error: {msg}"));
    }
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n");
    let value: Value = result
        .structured_content
        .clone()
        .ok_or_else(|| anyhow!("missing structured content"))?;
    let s: Structured = serde_json::from_value(value).context("parsing structured content")?;
    Ok((s, text))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(args.url.clone()),
    );
    let client = ().serve(transport).await.context("connecting to the MCP server")?;
    let tools = client.list_all_tools().await?;
    eprintln!(
        "connected to {}; tools: {}",
        args.url,
        tools
            .iter()
            .map(|t| t.name.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    for run_no in 1..=args.runs {
        let call = |name: &'static str, arguments: Value| {
            let client = &client;
            async move {
                let params = CallToolRequestParams::new(name)
                    .with_arguments(arguments.as_object().cloned().unwrap_or_default());
                let result = client
                    .call_tool(params)
                    .await
                    .with_context(|| format!("calling {name}"))?;
                parse(&result)
            }
        };
        let (mut s, text) = call(
            "start_run",
            json!({ "agent_name": args.agent_name, "set_name": args.set, "model": args.model }),
        )
        .await?;
        let run_id = s.run_id.clone();
        eprintln!("run {run_no}: id {run_id}");
        if args.verbose {
            println!("{text}");
        }
        let mut map = s
            .map
            .take()
            .ok_or_else(|| anyhow!("start_run returned no map"))?;
        let mut maze = maze_from(&map);
        let mut pilot = Pilot::for_maze(&maze, &maze.shortest_paths(), PilotParams::default());
        let mut turns = 0u32;
        loop {
            if s.status != "running" {
                break;
            }
            let state = s.state.as_ref().ok_or_else(|| anyhow!("missing state"))?;
            let cmd = pilot.command(&physics_state(state), &map.physics);
            let note = format!(
                "waypoint {} of {}",
                pilot.current_waypoint() + 1,
                pilot.waypoints().len()
            );
            let (next, text) = call(
                "act",
                json!({ "run_id": run_id, "rotate_deg": cmd.rotate_deg, "thrust": cmd.thrust, "note": note }),
            )
            .await?;
            turns += 1;
            if args.verbose {
                println!("{text}");
            }
            match next.event.as_deref() {
                Some("wall") | Some("turn_limit") => {
                    eprintln!(
                        "  map {}: {} after {turns} turns, retrying",
                        next.map_index + 1,
                        next.event.as_deref().unwrap_or("")
                    );
                    pilot.reset();
                }
                Some("goal") => eprintln!(
                    "  map {} done, score so far {}",
                    s.map_index + 1,
                    next.score
                ),
                _ => {}
            }
            if let Some(m) = &next.map {
                map = m.clone();
                maze = maze_from(&map);
                pilot = Pilot::for_maze(&maze, &maze.shortest_paths(), PilotParams::default());
            }
            s = next;
            if args.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(args.delay_ms)).await;
            }
        }
        eprintln!(
            "run {run_no} finished with score {} after {turns} turns",
            s.score
        );
        println!("{}", s.score);
    }
    client.cancel().await.ok();
    Ok(())
}
