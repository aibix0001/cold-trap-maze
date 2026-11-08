//! Cold Trap Maze server: simulation, MCP endpoint, WebSocket stream,
//! REST API and static files for the browser.

mod api;
mod db;
mod mcp;
mod state;

use anyhow::{Context, Result};
use axum::Router;
use clap::Parser;
use cold_trap_sim::set::MapSet;
use cold_trap_sim::GENERATOR_VERSION;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tower_http::services::{ServeDir, ServeFile};
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(name = "cold-trap-server", about = "Cold Trap Maze benchmark server")]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:8080", env = "COLD_TRAP_BIND")]
    bind: SocketAddr,
    /// SQLite database file.
    #[arg(long, default_value = "data/cold-trap.db", env = "COLD_TRAP_DB")]
    db: PathBuf,
    /// Directory with set files (*.json).
    #[arg(long, default_value = "sets", env = "COLD_TRAP_SETS")]
    sets: PathBuf,
    /// Directory with the built web frontend.
    #[arg(long, default_value = "web/dist", env = "COLD_TRAP_WEB")]
    web: PathBuf,
    /// Additional Host header values accepted on /mcp (DNS-rebinding
    /// protection). Loopback hosts and the bind address are always allowed.
    #[arg(long, value_delimiter = ',', env = "COLD_TRAP_ALLOWED_HOSTS")]
    allowed_hosts: Vec<String>,
    /// Accept any Host header on /mcp (only behind a trusted reverse proxy).
    #[arg(long, env = "COLD_TRAP_ANY_HOST")]
    any_host: bool,
}

fn load_sets(dir: &PathBuf) -> Result<HashMap<String, Arc<MapSet>>> {
    let mut sets = HashMap::new();
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("reading set directory {}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        match MapSet::from_json(&text) {
            Ok(set) => {
                if set.generator_version != GENERATOR_VERSION {
                    warn!(
                        "set {} ({}) was built with generator v{}, server has v{}: maps may differ from the file's intent",
                        set.name,
                        path.display(),
                        set.generator_version,
                        GENERATOR_VERSION
                    );
                }
                info!(
                    "loaded set '{}' with {} maps from {}",
                    set.name,
                    set.maps.len(),
                    path.display()
                );
                sets.insert(set.name.clone(), Arc::new(set));
            }
            Err(e) => warn!("skipping {}: {e}", path.display()),
        }
    }
    Ok(sets)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,rmcp=warn".into()),
        )
        .init();
    let args = Args::parse();

    let sets = load_sets(&args.sets)?;
    if sets.is_empty() {
        warn!(
            "no sets loaded from {}; build one with cold-trap-setbuilder",
            args.sets.display()
        );
    }
    let db = db::Db::open(&args.db)?;
    for set in sets.values() {
        db.store_set(set)?;
    }
    let state = state::AppState::new(sets, db);

    let ct = CancellationToken::new();
    let mut hosts: Vec<String> = vec![
        "localhost".into(),
        "127.0.0.1".into(),
        "[::1]".into(),
        args.bind.to_string(),
        args.bind.ip().to_string(),
    ];
    if args.bind.ip().is_unspecified() {
        hosts.push(format!("localhost:{}", args.bind.port()));
        hosts.push(format!("127.0.0.1:{}", args.bind.port()));
    }
    hosts.extend(args.allowed_hosts.iter().cloned());
    let mut mcp_config = StreamableHttpServerConfig::default()
        .with_cancellation_token(ct.child_token())
        .with_json_response(true)
        .with_allowed_hosts(hosts);
    if args.any_host {
        mcp_config = mcp_config.disable_allowed_hosts();
    }
    let mcp_state = state.clone();
    let mcp_service = StreamableHttpService::new(
        move || Ok(mcp::ColdTrapMcp::new(mcp_state.clone())),
        Arc::new(LocalSessionManager::default()),
        mcp_config,
    );

    let index = args.web.join("index.html");
    let static_files = ServeDir::new(&args.web).not_found_service(ServeFile::new(index));
    if !args.web.exists() {
        warn!(
            "web directory {} does not exist; build the frontend with `npm run build` in web/",
            args.web.display()
        );
    }

    let app = Router::new()
        .nest_service("/mcp", mcp_service)
        .merge(api::router(state.clone()))
        .fallback_service(static_files);

    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("binding {}", args.bind))?;
    info!(
        "listening on http://{}  (MCP endpoint: http://{}/mcp)",
        args.bind, args.bind
    );
    let shutdown_ct = ct.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            info!("shutting down");
            shutdown_ct.cancel();
        })
        .await?;
    Ok(())
}
