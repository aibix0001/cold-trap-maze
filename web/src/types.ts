// Wire types shared with the Rust server (see crates/server/src/state.rs).

export interface Vec2 {
  x: number;
  y: number;
}

export interface Cell {
  x: number;
  y: number;
}

export type Side = "top" | "right" | "bottom" | "left";

export interface PhysicsParams {
  dt: number;
  turn_seconds: number;
  gravity: number;
  max_thrust_accel: number;
  max_rotate_deg: number;
  ship_radius: number;
  load_radius: number;
  rope_length: number;
  load_mass_ratio: number;
  wall_half_thickness: number;
  frame_stride: number;
}

export interface MapView {
  index: number;
  seed: number;
  width: number;
  height: number;
  cell_size: number;
  h_walls: boolean[];
  v_walls: boolean[];
  start: Cell;
  exit: Cell;
  exit_side: Side;
  shortest_cells: Cell[];
  path: Cell[];
  shortest_length: number;
  physics: PhysicsParams;
  turn_limit: number;
  attempt_limit: number;
}

export interface Frame {
  t: number;
  ship: Vec2;
  load: Vec2;
  heading_deg: number;
}

export interface TurnState {
  ship: Vec2;
  ship_vel: Vec2;
  heading_deg: number;
  load: Vec2;
  load_vel: Vec2;
  rope_angle_deg: number;
  rope_angular_velocity_deg: number;
  rope_taut: boolean;
}

export type EventKind = "ok" | "wall" | "goal" | "turn_limit";

export interface TurnLog {
  map_index: number;
  attempt: number;
  turn: number;
  command: { rotate_deg: number; thrust: number };
  note: string | null;
  event: EventKind;
  frames: Frame[];
  state: TurnState;
  next: TurnState;
  created_at: string;
}

export interface MapResult {
  completed: boolean;
  attempts: number;
  turns: number;
  shortest_path: boolean;
}

export type RunStatus = "running" | "finished" | "aborted";

export interface RunSummary {
  id: string;
  agent_name: string;
  model: string;
  set_name: string;
  status: RunStatus;
  map_index: number;
  map_count: number;
  attempt: number;
  turn: number;
  total_turns: number;
  score: number;
  map_scores: (number | null)[];
  map_results: (MapResult | null)[];
  started_at: string;
  finished_at: string | null;
  last_note: string | null;
}

export type WsEvent =
  | { type: "snapshot"; runs: RunSummary[] }
  | { type: "run_started"; run: RunSummary; map: MapView }
  | { type: "turn"; run: RunSummary; turn: TurnLog }
  | { type: "map_started"; run: RunSummary; map: MapView }
  | { type: "map_finished"; run: RunSummary; map_index: number; result: MapResult; points: number }
  | { type: "run_finished"; run: RunSummary }
  | { type: "run_aborted"; run: RunSummary };

export interface RunDetail {
  run: RunSummary;
  maps: MapView[];
  turns: TurnLog[];
}

export interface LeaderboardEntry {
  set_name: string;
  model: string;
  agents: string[];
  runs: number;
  median_score: number;
  provisional: boolean;
  best_score: number;
  best_run_id: string;
  best_run_turns: number;
  maps_completed_best: number;
  latest: string;
  run_ids: string[];
}
