//! Benchmark sets: a named list of five seeds plus the parameters that
//! turn them into maps. A set file is a self-contained artefact that can
//! be shared; the same file always yields the same maps.

use crate::maze::{Difficulty, Maze, MazeParams};
use crate::physics::PhysicsParams;
use crate::score::ScoreWeights;
use serde::{Deserialize, Serialize};

/// One map of a set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapSpec {
    pub seed: u64,
    #[serde(flatten)]
    pub maze: MazeParams,
    /// Informal class label ("easy", "medium", "hard").
    #[serde(default)]
    pub class: String,
    /// Difficulty as measured by the set builder (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<Difficulty>,
    /// Turns the reference pilot needed (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pilot_turns: Option<u32>,
}

impl MapSpec {
    pub fn generate(&self) -> Maze {
        Maze::generate(self.seed, &self.maze)
    }
}

/// A complete set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapSet {
    pub name: String,
    pub generator_version: u32,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub physics: PhysicsParams,
    #[serde(default)]
    pub scoring: ScoreWeights,
    #[serde(default = "default_turn_limit")]
    pub turn_limit: u32,
    #[serde(default = "default_attempt_limit")]
    pub attempt_limit: u32,
    pub maps: Vec<MapSpec>,
}

fn default_turn_limit() -> u32 {
    300
}
fn default_attempt_limit() -> u32 {
    5
}

impl MapSet {
    pub fn from_json(s: &str) -> Result<MapSet, serde_json::Error> {
        serde_json::from_str(s)
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("set serialises")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let set = MapSet {
            name: "test".into(),
            generator_version: crate::GENERATOR_VERSION,
            description: String::new(),
            physics: PhysicsParams::default(),
            scoring: ScoreWeights::default(),
            turn_limit: 300,
            attempt_limit: 5,
            maps: vec![MapSpec {
                seed: 5,
                maze: MazeParams::new(8, 8, 0.0),
                class: "easy".into(),
                difficulty: None,
                pilot_turns: None,
            }],
        };
        let json = set.to_json();
        let back = MapSet::from_json(&json).unwrap();
        assert_eq!(set, back);
    }
}
