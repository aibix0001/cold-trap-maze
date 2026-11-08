//! SQLite persistence: sets, runs and turns.

use crate::state::{RunStatus, RunSummary, TurnLog};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use cold_trap_sim::score::MapResult;
use cold_trap_sim::set::MapSet;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;

pub struct Db {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sets (
    name TEXT PRIMARY KEY,
    json TEXT NOT NULL,
    loaded_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY,
    agent_name TEXT NOT NULL,
    model TEXT NOT NULL,
    set_name TEXT NOT NULL,
    status TEXT NOT NULL,
    map_index INTEGER NOT NULL,
    map_count INTEGER NOT NULL,
    total_turns INTEGER NOT NULL,
    score INTEGER NOT NULL,
    map_scores TEXT NOT NULL,
    map_results TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT
);
CREATE INDEX IF NOT EXISTS runs_set ON runs(set_name, status);
CREATE TABLE IF NOT EXISTS turns (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    map_index INTEGER NOT NULL,
    attempt INTEGER NOT NULL,
    turn INTEGER NOT NULL,
    json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS turns_run ON turns(run_id, id);
"#;

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(SCHEMA)?;
        // Runs that were live when the server last stopped can never resume.
        conn.execute("UPDATE runs SET status='aborted', finished_at=COALESCE(finished_at, ?1) WHERE status='running'", params![Utc::now().to_rfc3339()])?;
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Db> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    pub fn store_set(&self, set: &MapSet) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sets(name, json, loaded_at) VALUES (?1, ?2, ?3) ON CONFLICT(name) DO UPDATE SET json=excluded.json, loaded_at=excluded.loaded_at",
            params![set.name, set.to_json(), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn insert_run(&self, r: &RunSummary) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO runs(id, agent_name, model, set_name, status, map_index, map_count, total_turns, score, map_scores, map_results, started_at, finished_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                r.id.to_string(),
                r.agent_name,
                r.model,
                r.set_name,
                r.status.as_str(),
                r.map_index as i64,
                r.map_count as i64,
                r.total_turns as i64,
                r.score,
                serde_json::to_string(&r.map_scores)?,
                serde_json::to_string(&r.map_results)?,
                r.started_at.to_rfc3339(),
                r.finished_at.map(|t| t.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    pub fn update_run(&self, r: &RunSummary) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE runs SET status=?2, map_index=?3, total_turns=?4, score=?5, map_scores=?6, map_results=?7, finished_at=?8 WHERE id=?1",
            params![
                r.id.to_string(),
                r.status.as_str(),
                r.map_index as i64,
                r.total_turns as i64,
                r.score,
                serde_json::to_string(&r.map_scores)?,
                serde_json::to_string(&r.map_results)?,
                r.finished_at.map(|t| t.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    pub fn insert_turn(&self, run_id: &Uuid, t: &TurnLog) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO turns(run_id, map_index, attempt, turn, json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                run_id.to_string(),
                t.map_index as i64,
                t.attempt as i64,
                t.turn as i64,
                serde_json::to_string(t)?
            ],
        )?;
        Ok(())
    }

    fn row_to_summary(row: &rusqlite::Row) -> rusqlite::Result<RunSummary> {
        let id: String = row.get("id")?;
        let map_scores: String = row.get("map_scores")?;
        let map_results: String = row.get("map_results")?;
        let started: String = row.get("started_at")?;
        let finished: Option<String> = row.get("finished_at")?;
        let status: String = row.get("status")?;
        let map_scores: Vec<Option<i64>> = serde_json::from_str(&map_scores).unwrap_or_default();
        let map_results: Vec<Option<MapResult>> =
            serde_json::from_str(&map_results).unwrap_or_default();
        Ok(RunSummary {
            id: Uuid::parse_str(&id).unwrap_or_default(),
            agent_name: row.get("agent_name")?,
            model: row.get("model")?,
            set_name: row.get("set_name")?,
            status: RunStatus::parse(&status),
            map_index: row.get::<_, i64>("map_index")? as usize,
            map_count: row.get::<_, i64>("map_count")? as usize,
            attempt: 0,
            turn: 0,
            total_turns: row.get::<_, i64>("total_turns")? as u32,
            score: row.get("score")?,
            map_scores,
            map_results,
            started_at: parse_time(&started),
            finished_at: finished.as_deref().map(parse_time),
            last_note: None,
        })
    }

    /// Stored runs, newest first.
    pub fn list_runs(&self, limit: usize) -> Result<Vec<RunSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT * FROM runs ORDER BY started_at DESC LIMIT ?1")?;
        let rows = stmt.query_map(params![limit as i64], Self::row_to_summary)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn get_run(&self, id: &Uuid) -> Result<Option<RunSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT * FROM runs WHERE id=?1")?;
        Ok(stmt
            .query_row(params![id.to_string()], Self::row_to_summary)
            .optional()?)
    }

    pub fn list_turns(&self, id: &Uuid) -> Result<Vec<TurnLog>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT json FROM turns WHERE run_id=?1 ORDER BY id")?;
        let rows = stmt.query_map(params![id.to_string()], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            let json = r?;
            if let Ok(t) = serde_json::from_str::<TurnLog>(&json) {
                out.push(t);
            }
        }
        Ok(out)
    }

    /// All finished runs (for the leaderboard).
    pub fn finished_runs(&self) -> Result<Vec<RunSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT * FROM runs WHERE status='finished' ORDER BY started_at DESC")?;
        let rows = stmt.query_map([], Self::row_to_summary)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}

fn parse_time(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}
