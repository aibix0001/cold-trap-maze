//! Scoring. Points are awarded per map; a set's score is the sum.

use serde::{Deserialize, Serialize};

/// Scoring weights (starting values from the PRD).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreWeights {
    /// Points for completing a map.
    pub completed: i64,
    /// Deduction for every attempt after the first.
    pub extra_attempt: i64,
    /// Deduction per turn in the successful attempt.
    pub turn: i64,
    /// Bonus when the flown route only used shortest-path cells.
    pub shortest_path: i64,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self {
            completed: 1000,
            extra_attempt: 150,
            turn: 1,
            shortest_path: 250,
        }
    }
}

/// Outcome of one map, as needed for scoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapResult {
    pub completed: bool,
    /// Attempts used, including the successful one (or all failed ones).
    pub attempts: u32,
    /// Turns in the successful attempt; 0 if not completed.
    pub turns: u32,
    /// Every cell the ship flew through in the successful attempt lies on
    /// some shortest path.
    pub shortest_path: bool,
}

/// Points for one map. A map that was not completed scores 0.
pub fn score_map(r: &MapResult, w: &ScoreWeights) -> i64 {
    if !r.completed {
        return 0;
    }
    let mut s = w.completed;
    s -= w.extra_attempt * (r.attempts.saturating_sub(1)) as i64;
    s -= w.turn * r.turns as i64;
    if r.shortest_path {
        s += w.shortest_path;
    }
    s.max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores() {
        let w = ScoreWeights::default();
        assert_eq!(
            score_map(
                &MapResult {
                    completed: false,
                    attempts: 5,
                    turns: 0,
                    shortest_path: false
                },
                &w
            ),
            0
        );
        assert_eq!(
            score_map(
                &MapResult {
                    completed: true,
                    attempts: 1,
                    turns: 40,
                    shortest_path: true
                },
                &w
            ),
            1210
        );
        assert_eq!(
            score_map(
                &MapResult {
                    completed: true,
                    attempts: 3,
                    turns: 100,
                    shortest_path: false
                },
                &w
            ),
            600
        );
    }
}
