//! Set builder: tries seeds, measures difficulty, proves flyability with
//! the reference pilot and writes a five-map benchmark set.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use cold_trap_pilot::{fly, PilotParams};
use cold_trap_sim::game::{Game, GameConfig};
use cold_trap_sim::maze::{DifficultyWeights, Maze, MazeParams};
use cold_trap_sim::physics::PhysicsParams;
use cold_trap_sim::score::ScoreWeights;
use cold_trap_sim::set::{MapSet, MapSpec};
use cold_trap_sim::GENERATOR_VERSION;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "cold-trap-setbuilder",
    about = "Build and inspect Cold Trap Maze benchmark sets"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Build a new set and write it as JSON.
    Build {
        /// Set name (used by agents in start_run).
        #[arg(long, default_value = "default")]
        name: String,
        /// Output file.
        #[arg(long, default_value = "sets/default.json")]
        out: PathBuf,
        /// First seed to try.
        #[arg(long, default_value_t = 1)]
        seed_start: u64,
        /// Flyable candidates to collect per map slot before choosing.
        #[arg(long, default_value_t = 24)]
        candidates: usize,
        /// Turn limit per attempt.
        #[arg(long, default_value_t = 300)]
        turn_limit: u32,
        /// Attempt limit per map.
        #[arg(long, default_value_t = 5)]
        attempt_limit: u32,
        /// A seed is accepted only if the pilot needs at most this fraction
        /// of the turn limit, leaving headroom for less efficient agents.
        #[arg(long, default_value_t = 0.85)]
        max_pilot_fraction: f64,
        /// Free-text description stored in the set.
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Print the maps of a set with their shortest paths and metrics.
    Show {
        file: PathBuf,
        /// Also fly every map with the reference pilot and report turns.
        #[arg(long)]
        fly: bool,
    },
}

/// The five slots of a standard set (grid size and loop fraction).
const SLOTS: [(&str, usize, f64); 5] = [
    ("easy", 8, 0.0),
    ("easy", 10, 0.0),
    ("medium", 14, 0.05),
    ("medium", 18, 0.08),
    ("hard", 24, 0.15),
];

struct Candidate {
    spec: MapSpec,
    difficulty: f64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Build {
            name,
            out,
            seed_start,
            candidates,
            turn_limit,
            attempt_limit,
            max_pilot_fraction,
            description,
        } => build(
            name,
            out,
            seed_start,
            candidates,
            turn_limit,
            attempt_limit,
            max_pilot_fraction,
            description,
        ),
        Cmd::Show { file, fly: do_fly } => show(file, do_fly),
    }
}

#[allow(clippy::too_many_arguments)]
fn build(
    name: String,
    out: PathBuf,
    seed_start: u64,
    candidates: usize,
    turn_limit: u32,
    attempt_limit: u32,
    max_pilot_fraction: f64,
    description: String,
) -> Result<()> {
    let physics = PhysicsParams::default();
    let weights = DifficultyWeights::default();
    let config = GameConfig {
        physics: physics.clone(),
        turn_limit,
        attempt_limit,
    };
    let max_turns = (turn_limit as f64 * max_pilot_fraction) as u32;

    let mut chosen: Vec<MapSpec> = Vec::new();
    let mut previous_difficulty = f64::NEG_INFINITY;
    for (i, (class, size, loops)) in SLOTS.iter().enumerate() {
        let params = MazeParams::new(*size, *size, *loops);
        let mut found: Vec<Candidate> = Vec::new();
        let mut tried = 0usize;
        let mut rejected_unflyable = 0usize;
        let mut rejected_slow = 0usize;
        let mut seed = seed_start;
        while found.len() < candidates {
            let maze = Maze::generate(seed, &params);
            tried += 1;
            let difficulty = maze.difficulty(&weights);
            let mut game = Game::new(maze, config.clone());
            let report = fly(&mut game, PilotParams::default(), |_| {});
            if !report.completed || report.attempts != 1 {
                rejected_unflyable += 1;
            } else if report.turns > max_turns {
                rejected_slow += 1;
            } else {
                found.push(Candidate {
                    spec: MapSpec {
                        seed,
                        maze: params.clone(),
                        class: class.to_string(),
                        difficulty: Some(difficulty.clone()),
                        pilot_turns: Some(report.turns),
                    },
                    difficulty: difficulty.score,
                });
            }
            seed += 1;
            if tried > candidates * 200 {
                bail!(
                    "slot {}: could not find {} flyable seeds after {} tries",
                    i + 1,
                    candidates,
                    tried
                );
            }
        }
        found.sort_by(|a, b| a.difficulty.partial_cmp(&b.difficulty).unwrap());
        // Median candidate, but strictly harder than the previous map.
        let mut pick = found.len() / 2;
        if found[pick].difficulty <= previous_difficulty {
            pick = found
                .iter()
                .position(|c| c.difficulty > previous_difficulty)
                .unwrap_or(found.len() - 1);
        }
        let c = &found[pick];
        eprintln!(
            "slot {} ({class} {size}x{size}, loops {loops:.2}): tried {tried} seeds, {rejected_unflyable} unflyable, {rejected_slow} too slow; picked seed {} with difficulty {:.1} (pilot {} turns)",
            i + 1,
            c.spec.seed,
            c.difficulty,
            c.spec.pilot_turns.unwrap_or(0)
        );
        previous_difficulty = c.difficulty;
        chosen.push(c.spec.clone());
    }

    let set = MapSet {
        name,
        generator_version: GENERATOR_VERSION,
        description,
        physics,
        scoring: ScoreWeights::default(),
        turn_limit,
        attempt_limit,
        maps: chosen,
    };
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&out, set.to_json()).with_context(|| format!("writing {}", out.display()))?;
    eprintln!("wrote {}", out.display());
    Ok(())
}

fn show(file: PathBuf, do_fly: bool) -> Result<()> {
    let text =
        std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
    let set = MapSet::from_json(&text)?;
    if set.generator_version != GENERATOR_VERSION {
        eprintln!(
            "warning: set was built with generator version {}, this binary has {}",
            set.generator_version, GENERATOR_VERSION
        );
    }
    println!(
        "Set '{}' (generator v{}), {} maps, {} turns/attempt, {} attempts/map",
        set.name,
        set.generator_version,
        set.maps.len(),
        set.turn_limit,
        set.attempt_limit
    );
    let weights = DifficultyWeights::default();
    for (i, spec) in set.maps.iter().enumerate() {
        let maze = spec.generate();
        let sp = maze.shortest_paths();
        let d = maze.difficulty(&weights);
        println!(
            "\nMap {} [{}] seed {} {}x{} loops {:.2}: path {} cells, {} turns, {} branches, {} upward moves, difficulty {:.1}",
            i + 1, spec.class, spec.seed, maze.width, maze.height, spec.maze.loop_fraction,
            d.path_length, d.direction_changes, d.dead_ends, d.upward_moves, d.score
        );
        print!("{}", maze.render_ascii(None, None, Some(&sp)));
        if do_fly {
            let config = GameConfig {
                physics: set.physics.clone(),
                turn_limit: set.turn_limit,
                attempt_limit: set.attempt_limit,
            };
            let mut game = Game::new(maze, config);
            let r = fly(&mut game, PilotParams::default(), |_| {});
            println!(
                "pilot: {} in {} turns, {} attempt(s), shortest path {}",
                if r.completed { "completed" } else { "FAILED" },
                r.turns,
                r.attempts,
                r.shortest_path
            );
        }
    }
    Ok(())
}
