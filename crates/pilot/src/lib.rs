//! Reference pilot: a scripted controller that flies the BFS shortest path.
//!
//! It serves two purposes: the set builder uses it to prove that a map is
//! physically flyable, and it is the baseline entry in the leaderboard.
//! It only uses what any agent gets: the observation (position, velocity,
//! heading, rope state) and the map. It never touches the simulation
//! internals.

use cold_trap_sim::game::{Command, Event, Game, TurnRecord};
use cold_trap_sim::maze::{Maze, ShortestPaths};
use cold_trap_sim::physics::{heading_delta, vector_heading, PhysicsParams, PhysicsState};
use cold_trap_sim::vec2::Vec2;
use serde::{Deserialize, Serialize};

/// Controller tuning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PilotParams {
    /// Cruise speed in straight corridors (units/s).
    pub v_max: f64,
    /// Separate cap for descending, where only thrust can brake.
    pub v_down_max: f64,
    /// Speed at which corners are taken.
    pub v_corner: f64,
    /// Deceleration assumed when planning braking before a corner.
    pub brake_accel: f64,
    /// Velocity feedback gain (1/s).
    pub k_v: f64,
    /// Gain for lateral centring in the corridor (1/s).
    pub k_lateral: f64,
    /// Largest commanded acceleration of the ship+load system.
    pub a_limit: f64,
    /// Distance at which a waypoint counts as reached.
    pub waypoint_tolerance: f64,
    /// Waypoints are shifted upward by this amount so that the hanging
    /// load has as much room below as the ship has above.
    pub upward_bias: f64,
    /// Seconds of reaction time assumed when planning braking (heading
    /// changes take turns).
    pub plan_latency: f64,
    /// Gain on the load's velocity relative to the ship, to damp the swing.
    pub k_sway: f64,
}

impl Default for PilotParams {
    fn default() -> Self {
        Self {
            v_max: 2.0,
            v_down_max: 1.0,
            v_corner: 0.45,
            brake_accel: 0.5,
            k_v: 1.5,
            k_lateral: 1.5,
            a_limit: 1.5,
            plan_latency: 1.0,
            k_sway: 0.35,
            waypoint_tolerance: 0.4,
            upward_bias: 0.3,
        }
    }
}

/// The scripted pilot for one map.
#[derive(Debug, Clone)]
pub struct Pilot {
    waypoints: Vec<Vec2>,
    idx: usize,
    params: PilotParams,
}

impl Pilot {
    /// Build the waypoint list from the canonical shortest path, plus two
    /// points outside the exit so the ship keeps flying straight out.
    pub fn for_maze(maze: &Maze, shortest: &ShortestPaths, params: PilotParams) -> Self {
        let cs = maze.cell_size();
        let bias = Vec2::new(0.0, -params.upward_bias);
        let mut waypoints: Vec<Vec2> = shortest
            .path
            .iter()
            .map(|c| maze.cell_center(*c) + bias)
            .collect();
        let exit_center = maze.cell_center(maze.exit) + bias;
        let out = maze.exit_side.outward();
        waypoints.push(exit_center + out * (cs * 1.5));
        waypoints.push(exit_center + out * (cs * 4.0));
        let idx = 1.min(waypoints.len() - 1);
        Self {
            waypoints,
            idx,
            params,
        }
    }

    /// Restart from the first waypoint (after a crash reset).
    pub fn reset(&mut self) {
        self.idx = 1.min(self.waypoints.len() - 1);
    }

    pub fn waypoints(&self) -> &[Vec2] {
        &self.waypoints
    }

    /// Index of the waypoint currently steered for.
    pub fn current_waypoint(&self) -> usize {
        self.idx
    }

    fn advance_waypoints(&mut self, pos: Vec2) {
        while self.idx + 1 < self.waypoints.len() {
            let wp = self.waypoints[self.idx];
            let prev = self.waypoints[self.idx - 1];
            let seg = wp - prev;
            let passed = seg.length_sq() > 0.0 && (pos - prev).dot(seg) / seg.length_sq() >= 1.0;
            if pos.distance(wp) <= self.params.waypoint_tolerance || passed {
                self.idx += 1;
            } else {
                break;
            }
        }
    }

    /// Distance along the remaining polyline from `pos` to the next change
    /// of direction (or to the end).
    fn distance_to_corner(&self, pos: Vec2) -> f64 {
        let mut d = pos.distance(self.waypoints[self.idx]);
        let mut i = self.idx;
        while i + 1 < self.waypoints.len() {
            let a = (self.waypoints[i] - self.waypoints[i - 1]).normalized();
            let b = (self.waypoints[i + 1] - self.waypoints[i]).normalized();
            if a.dot(b) < 0.99 {
                break;
            }
            d += self.waypoints[i].distance(self.waypoints[i + 1]);
            i += 1;
        }
        d
    }

    /// Compute the command for the next turn.
    pub fn command(&mut self, state: &PhysicsState, physics: &PhysicsParams) -> Command {
        let p = self.params.clone();
        let pos = state.ship.pos;
        // Thrust moves the centre of mass; the swing only redistributes
        // velocity between ship and load. Feeding back the centre-of-mass
        // velocity keeps the controller from chasing the pendulum.
        let r = physics.load_mass_ratio;
        let vel = (state.ship.vel + state.load.vel * r) * (1.0 / (1.0 + r));
        let sway = state.load.vel - state.ship.vel;
        self.advance_waypoints(pos);
        let target = self.waypoints[self.idx];
        let prev = self.waypoints[self.idx - 1];
        let seg_dir = (target - prev).normalized();
        let to_target = target - pos;
        // Lateral offset from the corridor centre line.
        let lateral = to_target - seg_dir * to_target.dot(seg_dir);

        // Speed planning: brake before corners, cruise in straights. The
        // command acts for a whole turn, so plan from where the ship will
        // be after one turn at the current speed.
        let d_corner =
            (self.distance_to_corner(pos) - vel.dot(seg_dir).max(0.0) * p.plan_latency).max(0.0);
        let last_leg = self.idx + 1 >= self.waypoints.len();
        let v_corner = if last_leg { p.v_max } else { p.v_corner };
        let mut v_allowed = (v_corner * v_corner + 2.0 * p.brake_accel * d_corner)
            .sqrt()
            .min(p.v_max);
        if seg_dir.y > 0.5 {
            v_allowed = v_allowed.min(p.v_down_max);
        }
        let v_des = seg_dir * v_allowed + (lateral * p.k_lateral).clamp_length(0.6);

        // Acceleration of the ship+load system we want this turn.
        let mut a_des = ((v_des - vel) * p.k_v + sway * p.k_sway).clamp_length(p.a_limit);
        // Never ask for more downward acceleration than gravity provides,
        // so the thrust vector always keeps an upward component.
        let max_down = physics.gravity * 0.8;
        if a_des.y > max_down {
            a_des.y = max_down;
        }

        // Thrust the ship must produce (ship-mass units): the whole system
        // mass has to be accelerated against gravity.
        let mass_factor = 1.0 + physics.load_mass_ratio;
        let thrust_vec = Vec2::new(a_des.x, a_des.y - physics.gravity) * mass_factor;
        let desired_heading = vector_heading(thrust_vec);
        let delta = heading_delta(state.heading_deg, desired_heading);
        let rotate = delta.clamp(-physics.max_rotate_deg, physics.max_rotate_deg);
        // The heading sweeps during the turn; use the mid-turn heading.
        let mid = cold_trap_sim::physics::heading_vector(state.heading_deg + rotate / 2.0);
        let thrust = if mid.y < -0.3 {
            // Pointing up enough: match the vertical component exactly so
            // the ship neither sinks nor climbs while it is still rotating.
            (thrust_vec.y / (mid.y * physics.max_thrust_accel)).clamp(0.0, 1.0)
        } else {
            // Otherwise the best we can do is the projection.
            (mid.dot(thrust_vec) / physics.max_thrust_accel).clamp(0.0, 1.0)
        };
        Command {
            rotate_deg: rotate,
            thrust,
        }
    }
}

/// Outcome of flying one map with the pilot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlightReport {
    pub completed: bool,
    pub attempts: u32,
    /// Turns in the successful attempt (0 if not completed).
    pub turns: u32,
    pub shortest_path: bool,
    pub events: Vec<Event>,
}

/// Fly a map to its end (goal or attempts exhausted) and report.
/// `on_turn` receives every turn record, e.g. for logging.
pub fn fly(
    game: &mut Game,
    params: PilotParams,
    mut on_turn: impl FnMut(&TurnRecord),
) -> FlightReport {
    let mut pilot = Pilot::for_maze(game.maze(), game.shortest(), params);
    let physics = game.config().physics.clone();
    let mut events = Vec::new();
    while !game.is_finished() {
        let cmd = pilot.command(game.state(), &physics);
        let rec = game.act(cmd);
        events.push(rec.event);
        if matches!(rec.event, Event::Wall | Event::TurnLimit) {
            pilot.reset();
        }
        on_turn(&rec);
    }
    let r = game.result().expect("finished");
    FlightReport {
        completed: r.completed,
        attempts: r.attempts,
        turns: r.turns,
        shortest_path: r.shortest_path,
        events,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cold_trap_sim::game::GameConfig;
    use cold_trap_sim::maze::MazeParams;

    fn success_rate(
        w: usize,
        h: usize,
        loops: f64,
        seeds: std::ops::Range<u64>,
    ) -> (usize, usize, f64, u32) {
        let mut ok = 0;
        let mut first_try = 0;
        let mut turns_sum = 0u64;
        let mut max_turns = 0u32;
        for seed in seeds {
            let maze = Maze::generate(seed, &MazeParams::new(w, h, loops));
            let mut game = Game::new(maze, GameConfig::default());
            let r = fly(&mut game, PilotParams::default(), |_| {});
            if r.completed {
                ok += 1;
                turns_sum += r.turns as u64;
                max_turns = max_turns.max(r.turns);
                if r.attempts == 1 {
                    first_try += 1;
                }
            }
        }
        (
            ok,
            first_try,
            turns_sum as f64 / ok.max(1) as f64,
            max_turns,
        )
    }

    #[test]
    fn pilot_flies_small_mazes() {
        let (ok, first, avg, max) = success_rate(8, 8, 0.0, 0..30);
        println!("8x8: {ok}/30 completed, {first} first try, avg turns {avg:.1}, max {max}");
        assert!(ok >= 29, "pilot completed only {ok}/30 small mazes");
    }

    #[test]
    fn pilot_flies_large_mazes() {
        let (ok, first, avg, max) = success_rate(24, 24, 0.15, 0..15);
        println!("24x24: {ok}/15 completed, {first} first try, avg turns {avg:.1}, max {max}");
        assert!(ok >= 13, "pilot completed only {ok}/15 large mazes");
    }

    #[test]
    fn pilot_is_deterministic() {
        let maze = Maze::generate(5, &MazeParams::new(10, 10, 0.0));
        let run = || {
            let mut g = Game::new(maze.clone(), GameConfig::default());
            fly(&mut g, PilotParams::default(), |_| {})
        };
        assert_eq!(run(), run());
    }
}

#[cfg(test)]
mod debug_tests {
    use super::*;
    use cold_trap_sim::game::GameConfig;
    use cold_trap_sim::maze::MazeParams;

    fn trace(seed: u64, w: usize, h: usize, loops: f64, max_turns: usize, tail: usize) {
        let maze = Maze::generate(seed, &MazeParams::new(w, h, loops));
        let sp = maze.shortest_paths();
        let mut game = Game::new(maze.clone(), GameConfig::default());
        let mut pilot = Pilot::for_maze(game.maze(), game.shortest(), PilotParams::default());
        let physics = game.config().physics.clone();
        let mut lines = Vec::new();
        let mut crashed = false;
        for _ in 0..max_turns {
            if game.is_finished() {
                break;
            }
            let s = *game.state();
            let cmd = pilot.command(&s, &physics);
            let wp = pilot.current_waypoint();
            let rec = game.act(cmd);
            lines.push(format!(
                "wp{} pos({:.2},{:.2}) vel({:.2},{:.2}) hd{:.0} rope{:.0} | rot {:.0} thr {:.2} -> {:?} pos({:.2},{:.2}) load({:.2},{:.2})",
                wp, s.ship.pos.x, s.ship.pos.y, s.ship.vel.x, s.ship.vel.y, s.heading_deg, s.rope_angle_deg(),
                cmd.rotate_deg, cmd.thrust, rec.event, rec.state_after.ship.pos.x, rec.state_after.ship.pos.y,
                rec.state_after.load.pos.x, rec.state_after.load.pos.y
            ));
            if rec.event == Event::Wall {
                crashed = true;
                break;
            }
        }
        if crashed {
            println!("seed {seed}: CRASH after {} turns", lines.len());
            println!(
                "{}",
                maze.render_ascii(
                    game.maze()
                        .cell_at(pilot.waypoints()[pilot.current_waypoint()]),
                    None,
                    Some(&sp)
                )
            );
            for l in lines.iter().rev().take(tail).rev() {
                println!("{l}");
            }
        } else {
            println!("seed {seed}: ok in {} turns", lines.len());
        }
    }

    #[test]
    #[ignore]
    fn trace_one() {
        for seed in 0..8 {
            trace(seed, 8, 8, 0.0, 400, 8);
        }
    }

    fn rate(
        params: &PilotParams,
        w: usize,
        h: usize,
        loops: f64,
        seeds: std::ops::Range<u64>,
    ) -> (usize, usize, f64) {
        let n = seeds.end - seeds.start;
        let mut ok = 0;
        let mut turns = 0u64;
        for seed in seeds {
            let maze = Maze::generate(seed, &MazeParams::new(w, h, loops));
            let mut game = Game::new(
                maze,
                GameConfig {
                    turn_limit: 3000,
                    ..Default::default()
                },
            );
            let r = fly(&mut game, params.clone(), |_| {});
            if r.completed && r.attempts == 1 {
                ok += 1;
                turns += r.turns as u64;
            }
        }
        (ok, n as usize, turns as f64 / ok.max(1) as f64)
    }

    #[test]
    #[ignore]
    fn sweep() {
        let base = PilotParams::default();
        let mut combos = Vec::new();
        for &v_max in &[1.6, 2.0, 2.4] {
            for &v_corner in &[0.45, 0.7, 0.9] {
                for &a_limit in &[1.2, 1.5] {
                    for &k_sway in &[0.2, 0.35, 0.5] {
                        combos.push(PilotParams {
                            v_max,
                            v_corner,
                            brake_accel: 0.5,
                            a_limit,
                            k_sway,
                            ..base.clone()
                        });
                    }
                }
            }
        }
        for p in combos {
            let (a, an, at) = rate(&p, 8, 8, 0.0, 0..30);
            let (b, bn, bt) = rate(&p, 16, 16, 0.1, 0..15);
            let (c, cn, ct) = rate(&p, 24, 24, 0.15, 0..10);
            println!(
                "v_max {:.1} v_corner {:.2} a_lim {:.1} sway {:.2} | 8x8 {a}/{an} ({at:.0}) | 16x16 {b}/{bn} ({bt:.0}) | 24x24 {c}/{cn} ({ct:.0})",
                p.v_max, p.v_corner, p.a_limit, p.k_sway
            );
        }
    }
}
