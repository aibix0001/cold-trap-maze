//! Maze generation and analysis.
//!
//! A maze is a function of (seed, parameters). The generator is a
//! recursive backtracker (long corridors, few branches), optionally
//! followed by opening a fraction of the remaining interior walls to
//! create loops. Start and exit lie on opposite borders.

use crate::vec2::Vec2;
use rand::seq::SliceRandom;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Grid coordinates of a cell. `x` grows to the right, `y` grows downward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cell {
    pub x: usize,
    pub y: usize,
}

impl Cell {
    pub const fn new(x: usize, y: usize) -> Self {
        Self { x, y }
    }
}

/// A side of the maze rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

impl Side {
    pub fn opposite(self) -> Side {
        match self {
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
    /// Unit vector pointing out of the maze through this side.
    pub fn outward(self) -> Vec2 {
        match self {
            Side::Top => Vec2::new(0.0, -1.0),
            Side::Bottom => Vec2::new(0.0, 1.0),
            Side::Left => Vec2::new(-1.0, 0.0),
            Side::Right => Vec2::new(1.0, 0.0),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Side::Top => "top",
            Side::Right => "right",
            Side::Bottom => "bottom",
            Side::Left => "left",
        }
    }
}

/// Generator parameters. Together with the seed they fully define a maze.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MazeParams {
    pub width: usize,
    pub height: usize,
    /// Fraction (0..=1) of the interior walls left by the backtracker that
    /// are removed afterwards to create loops. 0 gives a perfect maze.
    #[serde(default)]
    pub loop_fraction: f64,
    /// Side length of one cell in world units. The ship diameter is 1.0,
    /// so the default of 3.0 is "three ship diameters".
    #[serde(default = "default_cell_size")]
    pub cell_size: f64,
}

fn default_cell_size() -> f64 {
    3.0
}

impl MazeParams {
    pub fn new(width: usize, height: usize, loop_fraction: f64) -> Self {
        Self {
            width,
            height,
            loop_fraction,
            cell_size: default_cell_size(),
        }
    }
}

impl Default for MazeParams {
    fn default() -> Self {
        Self::new(8, 8, 0.0)
    }
}

/// A generated maze. Walls are stored on the grid lines:
/// `h_walls[y * width + x]` is the wall on the *top* edge of cell (x, y),
/// for y in 0..=height; `v_walls[y * (width + 1) + x]` is the wall on the
/// *left* edge of cell (x, y), for x in 0..=width.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Maze {
    pub seed: u64,
    pub params: MazeParams,
    pub generator_version: u32,
    pub width: usize,
    pub height: usize,
    pub h_walls: Vec<bool>,
    pub v_walls: Vec<bool>,
    pub start: Cell,
    pub exit: Cell,
    pub exit_side: Side,
}

impl Maze {
    /// Generate the maze for `seed` and `params`.
    pub fn generate(seed: u64, params: &MazeParams) -> Maze {
        assert!(
            params.width >= 2 && params.height >= 2,
            "maze must be at least 2x2"
        );
        let (w, h) = (params.width, params.height);
        let mut rng = ChaCha8Rng::seed_from_u64(seed);

        let mut maze = Maze {
            seed,
            params: params.clone(),
            generator_version: crate::GENERATOR_VERSION,
            width: w,
            height: h,
            h_walls: vec![true; w * (h + 1)],
            v_walls: vec![true; (w + 1) * h],
            start: Cell::new(0, 0),
            exit: Cell::new(0, 0),
            exit_side: Side::Right,
        };

        // Recursive backtracker with an explicit stack.
        let mut visited = vec![false; w * h];
        let start_cell = Cell::new(rng.random_range(0..w), rng.random_range(0..h));
        let mut stack = vec![start_cell];
        visited[start_cell.y * w + start_cell.x] = true;
        while let Some(&cur) = stack.last() {
            let mut candidates: [Option<Cell>; 4] = [None; 4];
            let mut n = 0;
            if cur.y > 0 && !visited[(cur.y - 1) * w + cur.x] {
                candidates[n] = Some(Cell::new(cur.x, cur.y - 1));
                n += 1;
            }
            if cur.x + 1 < w && !visited[cur.y * w + cur.x + 1] {
                candidates[n] = Some(Cell::new(cur.x + 1, cur.y));
                n += 1;
            }
            if cur.y + 1 < h && !visited[(cur.y + 1) * w + cur.x] {
                candidates[n] = Some(Cell::new(cur.x, cur.y + 1));
                n += 1;
            }
            if cur.x > 0 && !visited[cur.y * w + cur.x - 1] {
                candidates[n] = Some(Cell::new(cur.x - 1, cur.y));
                n += 1;
            }
            if n == 0 {
                stack.pop();
                continue;
            }
            let next = candidates[rng.random_range(0..n)].unwrap();
            maze.remove_wall_between(cur, next);
            visited[next.y * w + next.x] = true;
            stack.push(next);
        }

        // Open loops: remove a fraction of the remaining interior walls.
        if params.loop_fraction > 0.0 {
            let mut interior: Vec<(bool, usize)> = Vec::new();
            for y in 1..h {
                for x in 0..w {
                    if maze.h_walls[y * w + x] {
                        interior.push((true, y * w + x));
                    }
                }
            }
            for y in 0..h {
                for x in 1..w {
                    if maze.v_walls[y * (w + 1) + x] {
                        interior.push((false, y * (w + 1) + x));
                    }
                }
            }
            interior.shuffle(&mut rng);
            let count =
                (params.loop_fraction.clamp(0.0, 1.0) * interior.len() as f64).round() as usize;
            for &(horizontal, idx) in interior.iter().take(count) {
                if horizontal {
                    maze.h_walls[idx] = false;
                } else {
                    maze.v_walls[idx] = false;
                }
            }
        }

        // Start and exit on opposite sides.
        let start_side = match rng.random_range(0..4u8) {
            0 => Side::Left,
            1 => Side::Right,
            2 => Side::Top,
            _ => Side::Bottom,
        };
        let exit_side = start_side.opposite();
        maze.start = maze.random_border_cell(&mut rng, start_side);
        maze.exit = maze.random_border_cell(&mut rng, exit_side);
        maze.exit_side = exit_side;
        maze.set_border_wall(maze.exit, exit_side, false);
        maze
    }

    fn random_border_cell(&self, rng: &mut ChaCha8Rng, side: Side) -> Cell {
        match side {
            Side::Left => Cell::new(0, rng.random_range(0..self.height)),
            Side::Right => Cell::new(self.width - 1, rng.random_range(0..self.height)),
            Side::Top => Cell::new(rng.random_range(0..self.width), 0),
            Side::Bottom => Cell::new(rng.random_range(0..self.width), self.height - 1),
        }
    }

    fn set_border_wall(&mut self, cell: Cell, side: Side, present: bool) {
        match side {
            Side::Top => self.h_walls[cell.y * self.width + cell.x] = present,
            Side::Bottom => self.h_walls[(cell.y + 1) * self.width + cell.x] = present,
            Side::Left => self.v_walls[cell.y * (self.width + 1) + cell.x] = present,
            Side::Right => self.v_walls[cell.y * (self.width + 1) + cell.x + 1] = present,
        }
    }

    fn remove_wall_between(&mut self, a: Cell, b: Cell) {
        if a.x == b.x {
            let y = a.y.max(b.y);
            self.h_walls[y * self.width + a.x] = false;
        } else {
            let x = a.x.max(b.x);
            self.v_walls[a.y * (self.width + 1) + x] = false;
        }
    }

    pub fn cell_size(&self) -> f64 {
        self.params.cell_size
    }

    /// Whether the wall on `side` of `cell` exists.
    pub fn has_wall(&self, cell: Cell, side: Side) -> bool {
        match side {
            Side::Top => self.h_walls[cell.y * self.width + cell.x],
            Side::Bottom => self.h_walls[(cell.y + 1) * self.width + cell.x],
            Side::Left => self.v_walls[cell.y * (self.width + 1) + cell.x],
            Side::Right => self.v_walls[cell.y * (self.width + 1) + cell.x + 1],
        }
    }

    /// Neighbouring cells reachable through an open wall.
    pub fn open_neighbours(&self, cell: Cell) -> Vec<Cell> {
        let mut out = Vec::with_capacity(4);
        if cell.y > 0 && !self.has_wall(cell, Side::Top) {
            out.push(Cell::new(cell.x, cell.y - 1));
        }
        if cell.x + 1 < self.width && !self.has_wall(cell, Side::Right) {
            out.push(Cell::new(cell.x + 1, cell.y));
        }
        if cell.y + 1 < self.height && !self.has_wall(cell, Side::Bottom) {
            out.push(Cell::new(cell.x, cell.y + 1));
        }
        if cell.x > 0 && !self.has_wall(cell, Side::Left) {
            out.push(Cell::new(cell.x - 1, cell.y));
        }
        out
    }

    /// World-space centre of a cell.
    pub fn cell_center(&self, cell: Cell) -> Vec2 {
        let cs = self.cell_size();
        Vec2::new((cell.x as f64 + 0.5) * cs, (cell.y as f64 + 0.5) * cs)
    }

    /// Cell containing a world position, if inside the maze.
    pub fn cell_at(&self, p: Vec2) -> Option<Cell> {
        let cs = self.cell_size();
        if p.x < 0.0 || p.y < 0.0 {
            return None;
        }
        let x = (p.x / cs).floor() as usize;
        let y = (p.y / cs).floor() as usize;
        if x < self.width && y < self.height {
            Some(Cell::new(x, y))
        } else {
            None
        }
    }

    /// World-space size of the maze rectangle.
    pub fn world_size(&self) -> Vec2 {
        Vec2::new(
            self.width as f64 * self.cell_size(),
            self.height as f64 * self.cell_size(),
        )
    }

    /// Point just outside the exit opening where a ship is considered "out".
    pub fn exit_point(&self) -> Vec2 {
        let c = self.cell_center(self.exit);
        c + self.exit_side.outward() * (self.cell_size() * 1.5)
    }

    /// Every wall as a world-space segment (merged per grid line segment,
    /// one segment per cell edge).
    pub fn wall_segments(&self) -> Vec<(Vec2, Vec2)> {
        let cs = self.cell_size();
        let mut out = Vec::new();
        for y in 0..=self.height {
            for x in 0..self.width {
                if self.h_walls[y * self.width + x] {
                    let a = Vec2::new(x as f64 * cs, y as f64 * cs);
                    let b = Vec2::new((x + 1) as f64 * cs, y as f64 * cs);
                    out.push((a, b));
                }
            }
        }
        for y in 0..self.height {
            for x in 0..=self.width {
                if self.v_walls[y * (self.width + 1) + x] {
                    let a = Vec2::new(x as f64 * cs, y as f64 * cs);
                    let b = Vec2::new(x as f64 * cs, (y + 1) as f64 * cs);
                    out.push((a, b));
                }
            }
        }
        out
    }

    fn bfs_from(&self, origin: Cell) -> Vec<Option<usize>> {
        let w = self.width;
        let mut dist = vec![None; w * self.height];
        let mut queue = VecDeque::new();
        dist[origin.y * w + origin.x] = Some(0);
        queue.push_back(origin);
        while let Some(c) = queue.pop_front() {
            let d = dist[c.y * w + c.x].unwrap();
            for n in self.open_neighbours(c) {
                if dist[n.y * w + n.x].is_none() {
                    dist[n.y * w + n.x] = Some(d + 1);
                    queue.push_back(n);
                }
            }
        }
        dist
    }

    /// Breadth-first shortest path analysis between start and exit.
    pub fn shortest_paths(&self) -> ShortestPaths {
        let w = self.width;
        let from_start = self.bfs_from(self.start);
        let to_exit = self.bfs_from(self.exit);
        let length = from_start[self.exit.y * w + self.exit.x].expect("maze is connected");
        let on_shortest: Vec<bool> = (0..w * self.height)
            .map(|i| matches!((from_start[i], to_exit[i]), (Some(a), Some(b)) if a + b == length))
            .collect();

        // One canonical path: walk from start, always to a neighbour whose
        // distance to the exit decreases (first in N,E,S,W order).
        let mut path = vec![self.start];
        let mut cur = self.start;
        while cur != self.exit {
            let d = to_exit[cur.y * w + cur.x].unwrap();
            let next = self
                .open_neighbours(cur)
                .into_iter()
                .find(|n| to_exit[n.y * w + n.x] == Some(d - 1))
                .expect("path continues");
            path.push(next);
            cur = next;
        }
        ShortestPaths {
            length,
            dist_from_start: from_start,
            dist_to_exit: to_exit,
            on_shortest,
            path,
        }
    }

    /// Difficulty metrics of the canonical shortest path.
    pub fn difficulty(&self, weights: &DifficultyWeights) -> Difficulty {
        let sp = self.shortest_paths();
        let path = &sp.path;
        let w = self.width;
        let mut direction_changes = 0usize;
        let mut upward_moves = 0usize;
        let mut last_dir: Option<(i64, i64)> = None;
        for pair in path.windows(2) {
            let dir = (
                pair[1].x as i64 - pair[0].x as i64,
                pair[1].y as i64 - pair[0].y as i64,
            );
            if dir.1 < 0 {
                upward_moves += 1;
            }
            if let Some(ld) = last_dir {
                if ld != dir {
                    direction_changes += 1;
                }
            }
            last_dir = Some(dir);
        }
        // Branches off the path: openings from a path cell to a non-path cell.
        let mut dead_ends = 0usize;
        for &c in path {
            for n in self.open_neighbours(c) {
                if !sp.on_shortest[n.y * w + n.x] {
                    dead_ends += 1;
                }
            }
        }
        let length = sp.length;
        let moves = length.max(1) as f64;
        let upward_fraction = upward_moves as f64 / moves;
        let score = weights.length * length as f64
            + weights.direction_change * direction_changes as f64
            + weights.dead_end * dead_ends as f64
            + weights.upward_move * upward_moves as f64;
        Difficulty {
            path_length: length,
            direction_changes,
            dead_ends,
            upward_moves,
            upward_fraction,
            score,
        }
    }

    /// ASCII rendering of the maze. Each cell is two characters wide and
    /// one high in the interior, surrounded by wall glyphs. The rows are
    /// labelled with their y index, the columns with their x index.
    /// Markers inside the cells: `S` start, `E` exit, `@` the ship's cell,
    /// `o` the load's cell (if different), `*` cells on a shortest path
    /// when `show_shortest` is set.
    pub fn render_ascii(
        &self,
        ship: Option<Cell>,
        load: Option<Cell>,
        show_shortest: Option<&ShortestPaths>,
    ) -> String {
        let (w, h) = (self.width, self.height);
        let mut out = String::new();
        let label_w = h.saturating_sub(1).to_string().len().max(1);
        let pad = " ".repeat(label_w + 1);
        // Column labels (tens and ones).
        if w > 10 {
            out.push_str(&pad);
            out.push(' ');
            for x in 0..w {
                let t = x / 10;
                out.push_str(&format!(
                    "{:>2}",
                    if t > 0 {
                        t.to_string()
                    } else {
                        " ".to_string()
                    }
                ));
            }
            out.push('\n');
        }
        out.push_str(&pad);
        out.push(' ');
        for x in 0..w {
            out.push_str(&format!("{:>2}", x % 10));
        }
        out.push('\n');
        for y in 0..h {
            // Top walls of row y.
            out.push_str(&pad);
            out.push('+');
            for x in 0..w {
                out.push_str(if self.h_walls[y * w + x] {
                    "--+"
                } else {
                    "  +"
                });
            }
            out.push('\n');
            // Cell row.
            out.push_str(&format!("{:>lw$} ", y, lw = label_w));
            for x in 0..w {
                let c = Cell::new(x, y);
                out.push(if self.has_wall(c, Side::Left) {
                    '|'
                } else {
                    ' '
                });
                let mut glyph = ' ';
                if let Some(sp) = show_shortest {
                    if sp.on_shortest[y * w + x] {
                        glyph = '*';
                    }
                }
                if c == self.start {
                    glyph = 'S';
                }
                if c == self.exit {
                    glyph = 'E';
                }
                if load == Some(c) {
                    glyph = 'o';
                }
                if ship == Some(c) {
                    glyph = '@';
                }
                out.push(glyph);
                out.push(' ');
            }
            out.push(if self.has_wall(Cell::new(w - 1, y), Side::Right) {
                '|'
            } else {
                ' '
            });
            out.push('\n');
        }
        out.push_str(&pad);
        out.push('+');
        for x in 0..w {
            out.push_str(if self.h_walls[h * w + x] {
                "--+"
            } else {
                "  +"
            });
        }
        out.push('\n');
        out
    }
}

/// Result of the BFS analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortestPaths {
    /// Number of moves from start to exit cell.
    pub length: usize,
    pub dist_from_start: Vec<Option<usize>>,
    pub dist_to_exit: Vec<Option<usize>>,
    /// Per cell: lies on at least one shortest path.
    pub on_shortest: Vec<bool>,
    /// One canonical shortest path, start to exit inclusive.
    pub path: Vec<Cell>,
}

impl ShortestPaths {
    pub fn is_on_shortest(&self, width: usize, cell: Cell) -> bool {
        self.on_shortest[cell.y * width + cell.x]
    }
}

/// Weights for the difficulty formula. Starting values; to be calibrated
/// with real agent runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DifficultyWeights {
    pub length: f64,
    pub direction_change: f64,
    pub dead_end: f64,
    pub upward_move: f64,
}

impl Default for DifficultyWeights {
    fn default() -> Self {
        Self {
            length: 1.0,
            direction_change: 2.0,
            dead_end: 1.5,
            upward_move: 3.0,
        }
    }
}

/// Measured difficulty of a maze.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Difficulty {
    pub path_length: usize,
    pub direction_changes: usize,
    pub dead_ends: usize,
    pub upward_moves: usize,
    pub upward_fraction: f64,
    pub score: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let p = MazeParams::new(10, 10, 0.1);
        let a = Maze::generate(42, &p);
        let b = Maze::generate(42, &p);
        assert_eq!(a, b);
        let c = Maze::generate(43, &p);
        assert_ne!(a, c);
    }

    #[test]
    fn perfect_maze_is_connected_and_has_unique_path() {
        for seed in 0..20u64 {
            let m = Maze::generate(seed, &MazeParams::new(8, 8, 0.0));
            let sp = m.shortest_paths();
            assert!(sp.dist_from_start.iter().all(|d| d.is_some()));
            assert_eq!(sp.path.first(), Some(&m.start));
            assert_eq!(sp.path.last(), Some(&m.exit));
            assert_eq!(sp.path.len(), sp.length + 1);
            // A perfect maze has exactly as many shortest-path cells as the path.
            assert_eq!(sp.on_shortest.iter().filter(|b| **b).count(), sp.length + 1);
            // The exit opening exists and start/exit are on opposite sides.
            assert!(!m.has_wall(m.exit, m.exit_side));
            match m.exit_side {
                Side::Right => assert_eq!((m.exit.x, m.start.x), (m.width - 1, 0)),
                Side::Left => assert_eq!((m.exit.x, m.start.x), (0, m.width - 1)),
                Side::Bottom => assert_eq!((m.exit.y, m.start.y), (m.height - 1, 0)),
                Side::Top => assert_eq!((m.exit.y, m.start.y), (0, m.height - 1)),
            }
        }
    }

    #[test]
    fn loops_open_walls() {
        let perfect = Maze::generate(7, &MazeParams::new(12, 12, 0.0));
        let loopy = Maze::generate(7, &MazeParams::new(12, 12, 0.2));
        let count = |m: &Maze| {
            m.h_walls
                .iter()
                .chain(m.v_walls.iter())
                .filter(|w| **w)
                .count()
        };
        assert!(count(&loopy) < count(&perfect));
    }

    #[test]
    fn ascii_renders() {
        let m = Maze::generate(1, &MazeParams::new(12, 5, 0.0));
        let s = m.render_ascii(Some(m.start), None, None);
        assert!(s.contains('@'));
        assert!(s.contains('E'));
        println!("{s}");
    }
}
