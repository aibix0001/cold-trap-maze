//! Game rules for one map: turns, attempts, resets, events and the text
//! observation an agent receives.

use crate::maze::{Cell, Maze, ShortestPaths};
use crate::physics::{self, Frame, PhysicsParams, PhysicsState, StepOutcome, Walls};
use crate::score::MapResult;
use crate::vec2::Vec2;
use serde::{Deserialize, Serialize};

/// Limits and physics for a map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameConfig {
    pub physics: PhysicsParams,
    pub turn_limit: u32,
    pub attempt_limit: u32,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            physics: PhysicsParams::default(),
            turn_limit: 300,
            attempt_limit: 5,
        }
    }
}

/// An agent command for one turn.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Command {
    /// Rotation in degrees, clockwise positive, clamped to the map limit.
    pub rotate_deg: f64,
    /// Thrust fraction 0..=1.
    pub thrust: f64,
}

/// Event reported after a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    /// Nothing special.
    Ok,
    /// Wall touched: attempt lost, state reset to the start.
    Wall,
    /// Exit passed with ship and load.
    Goal,
    /// Turn limit reached: attempt lost, state reset to the start.
    TurnLimit,
}

impl Event {
    pub fn name(self) -> &'static str {
        match self {
            Event::Ok => "ok",
            Event::Wall => "wall",
            Event::Goal => "goal",
            Event::TurnLimit => "turn_limit",
        }
    }
}

/// Everything recorded about one turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnRecord {
    /// Attempt number (1-based) the turn belonged to.
    pub attempt: u32,
    /// Turn number within the attempt (1-based).
    pub turn: u32,
    pub command: Command,
    pub event: Event,
    /// Animation frames of this turn.
    pub frames: Vec<Frame>,
    /// Physics state at the end of the turn (before any reset).
    pub state_after: PhysicsState,
}

/// State of one map for one agent.
#[derive(Debug, Clone)]
pub struct Game {
    maze: Maze,
    walls: Walls,
    shortest: ShortestPaths,
    config: GameConfig,
    state: PhysicsState,
    attempt: u32,
    turn: u32,
    cells_flown: Vec<Cell>,
    result: Option<MapResult>,
}

impl Game {
    pub fn new(maze: Maze, config: GameConfig) -> Self {
        let walls = Walls::from_maze(&maze);
        let shortest = maze.shortest_paths();
        let state = PhysicsState::at_rest(maze.cell_center(maze.start), &config.physics);
        let start = maze.start;
        Self {
            maze,
            walls,
            shortest,
            config,
            state,
            attempt: 1,
            turn: 0,
            cells_flown: vec![start],
            result: None,
        }
    }

    pub fn maze(&self) -> &Maze {
        &self.maze
    }
    pub fn shortest(&self) -> &ShortestPaths {
        &self.shortest
    }
    pub fn config(&self) -> &GameConfig {
        &self.config
    }
    pub fn state(&self) -> &PhysicsState {
        &self.state
    }
    pub fn attempt(&self) -> u32 {
        self.attempt
    }
    pub fn turn(&self) -> u32 {
        self.turn
    }
    /// Cells visited by the ship's centre in the current attempt.
    pub fn cells_flown(&self) -> &[Cell] {
        &self.cells_flown
    }
    /// Set once the map is finished (goal reached or attempts exhausted).
    pub fn result(&self) -> Option<MapResult> {
        self.result
    }
    pub fn is_finished(&self) -> bool {
        self.result.is_some()
    }

    fn reset_attempt(&mut self) {
        self.state =
            PhysicsState::at_rest(self.maze.cell_center(self.maze.start), &self.config.physics);
        self.turn = 0;
        self.cells_flown = vec![self.maze.start];
    }

    /// Execute one turn. Panics if the map is already finished.
    pub fn act(&mut self, cmd: Command) -> TurnRecord {
        assert!(self.result.is_none(), "map already finished");
        let attempt = self.attempt;
        self.turn += 1;
        let turn = self.turn;
        let sim = physics::simulate_turn(
            &mut self.state,
            &self.maze,
            &self.walls,
            &self.config.physics,
            cmd.rotate_deg,
            cmd.thrust,
        );
        for c in &sim.cells_visited {
            if self.cells_flown.last() != Some(c) {
                self.cells_flown.push(*c);
            }
        }
        let state_after = self.state;
        let event = match sim.outcome {
            StepOutcome::Goal => Event::Goal,
            StepOutcome::Wall => Event::Wall,
            StepOutcome::Ok if self.turn >= self.config.turn_limit => Event::TurnLimit,
            StepOutcome::Ok => Event::Ok,
        };
        match event {
            Event::Goal => {
                let shortest = self
                    .cells_flown
                    .iter()
                    .all(|c| self.shortest.is_on_shortest(self.maze.width, *c));
                self.result = Some(MapResult {
                    completed: true,
                    attempts: self.attempt,
                    turns: self.turn,
                    shortest_path: shortest,
                });
            }
            Event::Wall | Event::TurnLimit => {
                if self.attempt >= self.config.attempt_limit {
                    self.result = Some(MapResult {
                        completed: false,
                        attempts: self.attempt,
                        turns: 0,
                        shortest_path: false,
                    });
                } else {
                    self.attempt += 1;
                    self.reset_attempt();
                }
            }
            Event::Ok => {}
        }
        TurnRecord {
            attempt,
            turn,
            command: cmd,
            event,
            frames: sim.frames,
            state_after,
        }
    }

    /// Mark the map as given up (counts as not completed).
    pub fn give_up(&mut self) {
        if self.result.is_none() {
            self.result = Some(MapResult {
                completed: false,
                attempts: self.attempt,
                turns: 0,
                shortest_path: false,
            });
        }
    }

    /// Current observation.
    pub fn observation(&self) -> Observation {
        let p = &self.config.physics;
        let s = &self.state;
        let ship_cell = self.maze.cell_at(s.ship.pos);
        let load_cell = self.maze.cell_at(s.load.pos);
        Observation {
            map: self.maze.render_ascii(ship_cell, load_cell, None),
            width: self.maze.width,
            height: self.maze.height,
            cell_size: self.maze.cell_size(),
            start: self.maze.start,
            exit: self.maze.exit,
            exit_side: self.maze.exit_side.name().to_string(),
            ship: BodyObs {
                pos: s.ship.pos,
                vel: s.ship.vel,
                cell: ship_cell,
            },
            heading_deg: s.heading_deg,
            load: BodyObs {
                pos: s.load.pos,
                vel: s.load.vel,
                cell: load_cell,
            },
            rope_angle_deg: s.rope_angle_deg(),
            rope_angular_velocity_deg: s.rope_angular_velocity_deg(),
            rope_taut: s.rope_taut(p),
            rope_length: p.rope_length,
            turn: self.turn,
            turn_limit: self.config.turn_limit,
            attempt: self.attempt,
            attempt_limit: self.config.attempt_limit,
            gravity: p.gravity,
            max_thrust_accel: p.max_thrust_accel,
            max_rotate_deg: p.max_rotate_deg,
            turn_seconds: p.turn_seconds,
            ship_radius: p.ship_radius,
            load_radius: p.load_radius,
            load_mass_ratio: p.load_mass_ratio,
            hover_thrust: p.hover_thrust(),
        }
    }
}

/// Position and velocity of a body plus the cell it is in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodyObs {
    pub pos: Vec2,
    pub vel: Vec2,
    pub cell: Option<Cell>,
}

/// What the agent sees. `to_text` renders it as plain text; the struct
/// itself is also serialisable for the browser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub map: String,
    pub width: usize,
    pub height: usize,
    pub cell_size: f64,
    pub start: Cell,
    pub exit: Cell,
    pub exit_side: String,
    pub ship: BodyObs,
    pub heading_deg: f64,
    pub load: BodyObs,
    pub rope_angle_deg: f64,
    pub rope_angular_velocity_deg: f64,
    pub rope_taut: bool,
    pub rope_length: f64,
    pub turn: u32,
    pub turn_limit: u32,
    pub attempt: u32,
    pub attempt_limit: u32,
    pub gravity: f64,
    pub max_thrust_accel: f64,
    pub max_rotate_deg: f64,
    pub turn_seconds: f64,
    pub ship_radius: f64,
    pub load_radius: f64,
    pub load_mass_ratio: f64,
    pub hover_thrust: f64,
}

fn cell_str(c: Option<Cell>) -> String {
    match c {
        Some(c) => format!("({}, {})", c.x, c.y),
        None => "outside".to_string(),
    }
}

impl Observation {
    /// Plain-text observation for the agent.
    pub fn to_text(&self, header: &str) -> String {
        let mut t = String::new();
        if !header.is_empty() {
            t.push_str(header);
            t.push('\n');
        }
        t.push_str(&format!(
            "Maze {}x{} cells, cell size {:.1} units. Start S at ({}, {}), exit E at ({}, {}) opening to the {} border. @ = ship cell, o = load cell.\n",
            self.width, self.height, self.cell_size, self.start.x, self.start.y, self.exit.x, self.exit.y, self.exit_side
        ));
        t.push_str(&self.map);
        t.push_str(&format!(
            "World coordinates: x right, y DOWN (gravity +y). Cell (cx, cy) spans x in [{:.1}*cx, {:.1}*(cx+1)], y likewise.\n",
            self.cell_size, self.cell_size
        ));
        t.push_str(&format!(
            "Ship: pos ({:.3}, {:.3}) in cell {}, vel ({:.3}, {:.3}) u/s, heading {:.1} deg (0=up, 90=right, 180=down, 270=left).\n",
            self.ship.pos.x, self.ship.pos.y, cell_str(self.ship.cell), self.ship.vel.x, self.ship.vel.y, self.heading_deg
        ));
        t.push_str(&format!(
            "Load: pos ({:.3}, {:.3}) in cell {}, vel ({:.3}, {:.3}) u/s. Rope: angle {:.1} deg (0=hanging straight down, + = load to the right), angular velocity {:.1} deg/s, {}.\n",
            self.load.pos.x, self.load.pos.y, cell_str(self.load.cell), self.load.vel.x, self.load.vel.y,
            self.rope_angle_deg, self.rope_angular_velocity_deg, if self.rope_taut { "taut" } else { "slack" }
        ));
        t.push_str(&format!(
            "Turn {} of {}, attempt {} of {}.\n",
            self.turn, self.turn_limit, self.attempt, self.attempt_limit
        ));
        t.push_str(&format!(
            "Physics: each turn simulates {:.2} s; gravity {:.2} u/s^2; full thrust accelerates the ship alone at {:.2} u/s^2 (with the load hanging you need thrust {:.2} to hover); rotation per turn limited to +/-{:.0} deg, applied gradually over the turn; thrust only along the heading. Ship radius {:.2}, load radius {:.2}, rope length {:.2}. Touching a wall with ship, rope or load ends the attempt.\n",
            self.turn_seconds, self.gravity, self.max_thrust_accel, self.hover_thrust, self.max_rotate_deg,
            self.ship_radius, self.load_radius, self.rope_length
        ));
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maze::MazeParams;

    #[test]
    fn wall_resets_and_counts_attempts() {
        let maze = Maze::generate(3, &MazeParams::new(6, 6, 0.0));
        let mut g = Game::new(maze, GameConfig::default());
        let mut attempts_seen = 1;
        for _ in 0..1000 {
            if g.is_finished() {
                break;
            }
            let r = g.act(Command {
                rotate_deg: 0.0,
                thrust: 0.0,
            });
            if r.event == Event::Wall {
                attempts_seen += 1;
            }
        }
        let res = g.result().expect("finished after 5 attempts");
        assert!(!res.completed);
        assert_eq!(res.attempts, 5);
        assert_eq!(attempts_seen, 6);
    }

    #[test]
    fn turn_limit_resets() {
        let maze = Maze::generate(3, &MazeParams::new(6, 6, 0.0));
        let cfg = GameConfig {
            turn_limit: 3,
            ..Default::default()
        };
        let mut g = Game::new(maze, cfg);
        let hover = g.config().physics.hover_thrust();
        let r1 = g.act(Command {
            rotate_deg: 0.0,
            thrust: hover,
        });
        let r2 = g.act(Command {
            rotate_deg: 0.0,
            thrust: hover,
        });
        let r3 = g.act(Command {
            rotate_deg: 0.0,
            thrust: hover,
        });
        assert_eq!(
            (r1.event, r2.event, r3.event),
            (Event::Ok, Event::Ok, Event::TurnLimit)
        );
        assert_eq!(g.attempt(), 2);
        assert_eq!(g.turn(), 0);
    }

    #[test]
    fn observation_text_mentions_everything() {
        let maze = Maze::generate(3, &MazeParams::new(6, 6, 0.0));
        let g = Game::new(maze, GameConfig::default());
        let text = g.observation().to_text("Map 1 of 5");
        assert!(text.contains("Ship:"));
        assert!(text.contains("Load:"));
        assert!(text.contains("Turn 0 of 300"));
        println!("{text}");
    }
}
