//! Rigid-body physics for ship, rope and load.
//!
//! Coordinate conventions (shared with the browser canvas):
//! * world units, `y` grows downward, gravity points to `+y`;
//! * the ship's heading is measured in degrees, `0` = up (against
//!   gravity), `90` = right, `180` = down, `270` = left; positive rotation
//!   is clockwise on screen;
//! * thrust acts only along the heading, never backwards.
//!
//! The rope is a true rope: it only pulls when taut and goes slack
//! otherwise. Ship and load are integrated as separate point masses with
//! semi-implicit Euler; the rope is a distance constraint solved by
//! position projection plus an inelastic velocity impulse.

use crate::maze::Maze;
use crate::vec2::{point_segment_distance, segment_segment_distance, Vec2};
use serde::{Deserialize, Serialize};

/// Physical constants and limits of a map. Starting values from the PRD;
/// everything is a parameter so sets can be calibrated later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhysicsParams {
    /// Fixed integration step in seconds (1/120).
    pub dt: f64,
    /// Simulated seconds per turn (0.5).
    pub turn_seconds: f64,
    /// Gravity in world units per second squared.
    pub gravity: f64,
    /// Acceleration of the ship alone at full thrust.
    pub max_thrust_accel: f64,
    /// Largest rotation an agent may request per turn, in degrees.
    pub max_rotate_deg: f64,
    pub ship_radius: f64,
    pub load_radius: f64,
    /// Rope length (ship centre to load centre).
    pub rope_length: f64,
    /// Load mass divided by ship mass.
    pub load_mass_ratio: f64,
    /// Half thickness of a wall segment for collision purposes.
    pub wall_half_thickness: f64,
    /// Record a frame every this many integration steps.
    pub frame_stride: usize,
}

impl Default for PhysicsParams {
    fn default() -> Self {
        Self {
            dt: 1.0 / 120.0,
            turn_seconds: 0.5,
            gravity: 2.0,
            max_thrust_accel: 6.0,
            max_rotate_deg: 45.0,
            ship_radius: 0.5,
            load_radius: 0.15,
            rope_length: 0.7,
            load_mass_ratio: 0.3,
            wall_half_thickness: 0.1,
            frame_stride: 4,
        }
    }
}

impl PhysicsParams {
    /// Number of integration steps per turn.
    pub fn steps_per_turn(&self) -> usize {
        (self.turn_seconds / self.dt).round() as usize
    }
    /// Thrust fraction needed to hover with the load hanging.
    pub fn hover_thrust(&self) -> f64 {
        self.gravity * (1.0 + self.load_mass_ratio) / self.max_thrust_accel
    }
}

/// A point mass.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Body {
    pub pos: Vec2,
    pub vel: Vec2,
}

/// Complete dynamic state.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PhysicsState {
    pub ship: Body,
    pub load: Body,
    /// Heading in degrees, see module docs.
    pub heading_deg: f64,
    /// Simulated time since the start of the attempt.
    pub time: f64,
}

impl PhysicsState {
    /// Ship at rest at `ship_pos`, load hanging straight down.
    pub fn at_rest(ship_pos: Vec2, params: &PhysicsParams) -> Self {
        Self {
            ship: Body {
                pos: ship_pos,
                vel: Vec2::ZERO,
            },
            load: Body {
                pos: ship_pos + Vec2::new(0.0, params.rope_length),
                vel: Vec2::ZERO,
            },
            heading_deg: 0.0,
            time: 0.0,
        }
    }

    /// Rope angle in degrees: 0 when the load hangs straight below the
    /// ship, positive when the load is to the right.
    pub fn rope_angle_deg(&self) -> f64 {
        let d = self.load.pos - self.ship.pos;
        d.x.atan2(d.y).to_degrees()
    }

    /// Time derivative of [`rope_angle_deg`](Self::rope_angle_deg) in degrees per second.
    pub fn rope_angular_velocity_deg(&self) -> f64 {
        let d = self.load.pos - self.ship.pos;
        let v = self.load.vel - self.ship.vel;
        let l2 = d.length_sq();
        if l2 == 0.0 {
            return 0.0;
        }
        ((d.y * v.x - d.x * v.y) / l2).to_degrees()
    }

    /// Whether the rope is (nearly) taut.
    pub fn rope_taut(&self, params: &PhysicsParams) -> bool {
        self.load.pos.distance(self.ship.pos) >= params.rope_length - 1e-6
    }

    /// Unit vector of the heading.
    pub fn heading_vector(&self) -> Vec2 {
        heading_vector(self.heading_deg)
    }
}

/// Unit vector for a heading in degrees (0 = up, 90 = right).
pub fn heading_vector(heading_deg: f64) -> Vec2 {
    let r = heading_deg.to_radians();
    Vec2::new(r.sin(), -r.cos())
}

/// Heading in degrees for a direction vector (inverse of [`heading_vector`]).
pub fn vector_heading(v: Vec2) -> f64 {
    normalize_heading(v.x.atan2(-v.y).to_degrees())
}

/// Wrap a heading into `[0, 360)`.
pub fn normalize_heading(deg: f64) -> f64 {
    let d = deg % 360.0;
    if d < 0.0 {
        d + 360.0
    } else {
        d
    }
}

/// Smallest signed difference `to - from` in degrees, in `(-180, 180]`.
pub fn heading_delta(from: f64, to: f64) -> f64 {
    let mut d = (to - from) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    }
    if d <= -180.0 {
        d += 360.0;
    }
    d
}

/// One recorded animation frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub t: f64,
    pub ship: Vec2,
    pub load: Vec2,
    pub heading_deg: f64,
}

impl Frame {
    fn from_state(s: &PhysicsState) -> Self {
        Frame {
            t: s.time,
            ship: s.ship.pos,
            load: s.load.pos,
            heading_deg: s.heading_deg,
        }
    }
}

/// What happened during the integration of one turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepOutcome {
    /// Nothing special, the turn ran to completion.
    Ok,
    /// Ship, rope or load touched a wall; integration stopped there.
    Wall,
    /// Ship and load have both left the maze through the exit.
    Goal,
}

/// Precomputed collision geometry for a maze.
#[derive(Debug, Clone)]
pub struct Walls {
    segments: Vec<(Vec2, Vec2)>,
    /// Axis-aligned bounds of each segment, for cheap rejection.
    bounds: Vec<(Vec2, Vec2)>,
}

impl Walls {
    pub fn from_maze(maze: &Maze) -> Self {
        let segments = maze.wall_segments();
        let bounds = segments
            .iter()
            .map(|(a, b)| {
                (
                    Vec2::new(a.x.min(b.x), a.y.min(b.y)),
                    Vec2::new(a.x.max(b.x), a.y.max(b.y)),
                )
            })
            .collect();
        Self { segments, bounds }
    }

    pub fn segments(&self) -> &[(Vec2, Vec2)] {
        &self.segments
    }

    fn circle_hits(&self, c: Vec2, r: f64) -> bool {
        for (i, (a, b)) in self.segments.iter().enumerate() {
            let (lo, hi) = self.bounds[i];
            if c.x + r < lo.x || c.x - r > hi.x || c.y + r < lo.y || c.y - r > hi.y {
                continue;
            }
            if point_segment_distance(c, *a, *b) < r {
                return true;
            }
        }
        false
    }

    fn segment_hits(&self, p: Vec2, q: Vec2, r: f64) -> bool {
        let (plo, phi) = (
            Vec2::new(p.x.min(q.x) - r, p.y.min(q.y) - r),
            Vec2::new(p.x.max(q.x) + r, p.y.max(q.y) + r),
        );
        for (i, (a, b)) in self.segments.iter().enumerate() {
            let (lo, hi) = self.bounds[i];
            if phi.x < lo.x || plo.x > hi.x || phi.y < lo.y || plo.y > hi.y {
                continue;
            }
            if segment_segment_distance(p, q, *a, *b) < r {
                return true;
            }
        }
        false
    }

    /// True if ship, rope or load touch any wall.
    pub fn collides(&self, s: &PhysicsState, params: &PhysicsParams) -> bool {
        let ht = params.wall_half_thickness;
        self.circle_hits(s.ship.pos, params.ship_radius + ht)
            || self.circle_hits(s.load.pos, params.load_radius + ht)
            || self.segment_hits(s.ship.pos, s.load.pos, ht)
    }
}

/// Goal test: ship and load have fully passed the exit side's border line.
pub fn passed_exit(maze: &Maze, s: &PhysicsState, params: &PhysicsParams) -> bool {
    let size = maze.world_size();
    let beyond = |p: Vec2, r: f64| -> bool {
        match maze.exit_side {
            crate::maze::Side::Right => p.x - r > size.x,
            crate::maze::Side::Left => p.x + r < 0.0,
            crate::maze::Side::Bottom => p.y - r > size.y,
            crate::maze::Side::Top => p.y + r < 0.0,
        }
    };
    beyond(s.ship.pos, params.ship_radius) && beyond(s.load.pos, params.load_radius)
}

/// Advance the state by one integration step with constant thrust
/// fraction and a heading increment.
pub fn integrate_step(
    s: &mut PhysicsState,
    params: &PhysicsParams,
    thrust: f64,
    heading_step_deg: f64,
) {
    let dt = params.dt;
    s.heading_deg = normalize_heading(s.heading_deg + heading_step_deg);
    let g = Vec2::new(0.0, params.gravity);
    let thrust_acc = heading_vector(s.heading_deg) * (thrust * params.max_thrust_accel);

    // Semi-implicit Euler for both bodies.
    s.ship.vel += (g + thrust_acc) * dt;
    s.ship.pos += s.ship.vel * dt;
    s.load.vel += g * dt;
    s.load.pos += s.load.vel * dt;

    // Rope constraint: only when taut.
    let d = s.load.pos - s.ship.pos;
    let dist = d.length();
    if dist > params.rope_length && dist > 0.0 {
        let n = d * (1.0 / dist);
        let m_ship = 1.0;
        let m_load = params.load_mass_ratio;
        let w_ship = m_load / (m_ship + m_load);
        let w_load = m_ship / (m_ship + m_load);
        let excess = dist - params.rope_length;
        s.ship.pos += n * (excess * w_ship);
        s.load.pos -= n * (excess * w_load);
        let v_rel = (s.load.vel - s.ship.vel).dot(n);
        if v_rel > 0.0 {
            s.ship.vel += n * (v_rel * w_ship);
            s.load.vel -= n * (v_rel * w_load);
        }
    }
    s.time += dt;
}

/// Result of simulating one turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnSim {
    pub outcome: StepOutcome,
    /// Frames sampled during the turn, always including the final state.
    pub frames: Vec<Frame>,
    /// Cells the ship's centre was in during the turn, in order, deduplicated.
    pub cells_visited: Vec<crate::maze::Cell>,
}

/// Simulate one full turn: rotate by `rotate_deg` spread linearly over the
/// turn and apply `thrust` (0..=1) along the heading. Stops early at a
/// wall contact or when the goal is reached.
pub fn simulate_turn(
    s: &mut PhysicsState,
    maze: &Maze,
    walls: &Walls,
    params: &PhysicsParams,
    rotate_deg: f64,
    thrust: f64,
) -> TurnSim {
    let rotate = rotate_deg.clamp(-params.max_rotate_deg, params.max_rotate_deg);
    let thrust = if thrust.is_finite() {
        thrust.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let steps = params.steps_per_turn();
    let heading_step = rotate / steps as f64;
    let mut frames = Vec::with_capacity(steps / params.frame_stride.max(1) + 2);
    let mut cells = Vec::new();
    if let Some(c) = maze.cell_at(s.ship.pos) {
        cells.push(c);
    }
    let mut outcome = StepOutcome::Ok;
    for i in 1..=steps {
        integrate_step(s, params, thrust, heading_step);
        if let Some(c) = maze.cell_at(s.ship.pos) {
            if cells.last() != Some(&c) {
                cells.push(c);
            }
        }
        if walls.collides(s, params) {
            outcome = StepOutcome::Wall;
            frames.push(Frame::from_state(s));
            break;
        }
        if passed_exit(maze, s, params) {
            outcome = StepOutcome::Goal;
            frames.push(Frame::from_state(s));
            break;
        }
        if i % params.frame_stride.max(1) == 0 || i == steps {
            frames.push(Frame::from_state(s));
        }
    }
    TurnSim {
        outcome,
        frames,
        cells_visited: cells,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maze::MazeParams;

    #[test]
    fn heading_conversions() {
        for h in [0.0, 45.0, 90.0, 180.0, 270.0, 359.0] {
            let v = heading_vector(h);
            assert!((heading_delta(vector_heading(v), h)).abs() < 1e-9, "h={h}");
        }
        assert!((heading_delta(350.0, 10.0) - 20.0).abs() < 1e-12);
        assert!((heading_delta(10.0, 350.0) + 20.0).abs() < 1e-12);
    }

    #[test]
    fn free_fall_with_slack_rope_keeps_rope_slack_and_load_pendulum_is_stable() {
        let params = PhysicsParams::default();
        let mut s = PhysicsState::at_rest(Vec2::new(0.0, 0.0), &params);
        // Push the load sideways so it swings.
        s.load.vel = Vec2::new(1.0, 0.0);
        let mut max_len: f64 = 0.0;
        for _ in 0..1200 {
            integrate_step(&mut s, &params, params.hover_thrust(), 0.0);
            max_len = max_len.max(s.load.pos.distance(s.ship.pos));
        }
        assert!(
            max_len <= params.rope_length + 1e-6,
            "rope stretched: {max_len}"
        );
        // Hovering: the ship should not have drifted far vertically.
        assert!(s.ship.pos.y.abs() < 2.0, "ship drifted to {:?}", s.ship.pos);
    }

    #[test]
    fn hover_thrust_balances_gravity() {
        let params = PhysicsParams::default();
        let mut s = PhysicsState::at_rest(Vec2::ZERO, &params);
        for _ in 0..600 {
            integrate_step(&mut s, &params, params.hover_thrust(), 0.0);
        }
        assert!(s.ship.pos.y.abs() < 1e-3, "{:?}", s.ship.pos);
        assert!(s.ship.vel.y.abs() < 1e-6);
    }

    #[test]
    fn falling_into_floor_hits_wall() {
        let maze = Maze::generate(3, &MazeParams::new(6, 6, 0.0));
        let walls = Walls::from_maze(&maze);
        let params = PhysicsParams::default();
        let mut s = PhysicsState::at_rest(maze.cell_center(maze.start), &params);
        assert!(!walls.collides(&s, &params));
        let mut hit = false;
        for _ in 0..20 {
            let r = simulate_turn(&mut s, &maze, &walls, &params, 0.0, 0.0);
            if r.outcome == StepOutcome::Wall {
                hit = true;
                break;
            }
        }
        assert!(hit);
    }

    #[test]
    fn determinism() {
        let maze = Maze::generate(11, &MazeParams::new(8, 8, 0.0));
        let walls = Walls::from_maze(&maze);
        let params = PhysicsParams::default();
        let run = || {
            let mut s = PhysicsState::at_rest(maze.cell_center(maze.start), &params);
            let mut out = Vec::new();
            for i in 0..10 {
                let r = simulate_turn(&mut s, &maze, &walls, &params, (i as f64) * 7.0 - 20.0, 0.7);
                out.push((s, r.outcome));
            }
            out
        };
        assert_eq!(run(), run());
    }
}
