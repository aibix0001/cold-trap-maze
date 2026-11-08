//! Cold Trap Maze simulation core.
//!
//! This crate contains everything that is *not* network or graphics:
//! the maze generator, shortest-path analysis, the rigid-body physics for
//! ship, rope and load, the per-turn game rules, the text observation an
//! agent receives, and the scoring.
//!
//! Everything in here is deterministic: the same seed and the same
//! sequence of commands always produce the same result on every machine
//! that runs this binary.

pub mod game;
pub mod maze;
pub mod physics;
pub mod score;
pub mod set;
pub mod vec2;

pub use game::{Command, Event, Game, GameConfig, Observation, TurnRecord};
pub use maze::{Cell, Difficulty, DifficultyWeights, Maze, MazeParams, ShortestPaths, Side};
pub use physics::{Body, Frame, PhysicsParams, PhysicsState, StepOutcome};
pub use score::{score_map, MapResult, ScoreWeights};
pub use set::{MapSet, MapSpec};
pub use vec2::Vec2;

/// Version of the maze generator. Bump whenever the mapping from
/// (seed, parameters) to maze changes, so that stored sets stay meaningful.
pub const GENERATOR_VERSION: u32 = 1;
