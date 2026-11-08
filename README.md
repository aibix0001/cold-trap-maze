# Cold Trap Maze – Agent Benchmark

Cold Trap Maze is a turn-based thrust labyrinth used as a benchmark for LLM
agents. An agent steers a ship turn by turn through an MCP interface; humans
watch in the browser. The benchmark measures which model gets out of the maze
with how few attempts and turns, and whether it finds the shortest path.

The physics is real: the ship rotates, thrusts only forward, falls under
gravity and carries a load on a rope that swings. Touching a wall with ship,
rope or load ends the attempt. Mazes are generated from a seed and are fully
reproducible.

```
 Agent app A ──MCP──┐                         ┌──WebSocket──▶ Browser (watch, telemetry, replay)
 Agent app B ──MCP──┼──▶  cold-trap-server  ──┤
 Reference pilot ───┘     (Rust: simulation,  └──REST────────▶ leaderboard, replays
                           mazes, scoring,
                           SQLite log)
```

Only the server computes. Agents talk MCP and know nothing but four tools;
browsers receive the same state as a stream and never intervene.

## Repository layout

| Path | What it is |
|------|------------|
| `crates/sim` | `cold-trap-sim`: maze generator, BFS analysis, physics, game rules, observation text, scoring, set file format. No network, no graphics. |
| `crates/pilot` | `cold-trap-pilot`: the scripted reference pilot that flies the BFS path. Used by the set builder and as the baseline in the leaderboard. |
| `crates/setbuilder` | `cold-trap-setbuilder`: tries seeds, measures difficulty, proves flyability with the pilot and writes a five-map set. |
| `crates/server` | `cold-trap-server`: axum + rmcp. MCP endpoint, run engine, SQLite, WebSocket stream, REST API, static files. Also `cold-trap-pilot-client`, the reference pilot as an ordinary MCP client. |
| `web/` | Spectator frontend in plain TypeScript with Canvas 2D (no framework). |
| `sets/` | Benchmark sets as JSON. `sets/default.json` ships with the repo. |

## Quick start

Requirements: a stable Rust toolchain (1.88 or newer for rmcp 3.x) and Node 22
for the frontend build.

```bash
# 1. build everything
cargo build --release
(cd web && npm ci && npm run build)

# 2. start the server (MCP at /mcp, browser at /)
./target/release/cold-trap-server --bind 127.0.0.1:8080

# 3. in another terminal: fly the reference pilot over MCP as a baseline
./target/release/cold-trap-pilot-client --url http://127.0.0.1:8080/mcp --set default --delay-ms 300
```

Open <http://127.0.0.1:8080/> to watch. `--delay-ms` slows the pilot down so
the animation is visible; without it a full run takes about a second.

To let Claude Code fly a run, register the server as an MCP server and ask it
to play:

```bash
claude mcp add --transport http cold-trap http://127.0.0.1:8080/mcp
```

Any other MCP client works the same way; the server speaks the MCP
specification 2026-07-28 (stateless streamable HTTP) and still accepts older
clients that use the `initialize` handshake.

### Server options

| Flag | Default | Meaning |
|------|---------|---------|
| `--bind` | `127.0.0.1:8080` | Listen address. |
| `--db` | `data/cold-trap.db` | SQLite file (sets, runs, turns). |
| `--sets` | `sets` | Directory with set files (`*.json`). |
| `--web` | `web/dist` | Built frontend. |
| `--allowed-hosts a,b` | – | Extra `Host` header values accepted on `/mcp`. Loopback and the bind address are always accepted (DNS rebinding protection from rmcp). |
| `--any-host` | off | Accept any `Host` (only behind a trusted reverse proxy). |

All flags can also be given as environment variables (`COLD_TRAP_BIND`, …).

## The game

A turn is one command plus a fixed span of simulated time. The agent gives a
rotation in degrees (limited, start value ±45°) and a thrust fraction between
0 and 1. The server then simulates 0.5 s with a fixed step of 1/120 s and
returns the new state. Same inputs on the same map always give the same
result.

| Element | Rule |
|---------|------|
| Ship | Point mass with a heading. Gravity acts constantly downward. Thrust only along the heading, no reverse. Braking means turning and thrusting. |
| Load | Mass on a rope of fixed length, hanging from the start. The rope pulls when taut and goes slack otherwise; the load swings freely. Rope and load take part in collisions. |
| Wall | If ship, rope or load touches a wall the attempt is lost: reset to the start, attempt counter plus one. |
| Start | Ship hovers at the start cell with zero velocity, load hanging straight below. |
| Goal | Ship and load have both passed the exit. The exit is on the border opposite the start, not necessarily at the top. |
| Turn limit | An attempt also ends after 300 turns (start value). This replaces fuel. |
| Attempt limit | After 5 attempts without success the map counts as failed. |

No guns, no doors, no shields. The maze and the physics are the opponent.

### Coordinates and units

* World units; the ship has diameter 1.0, a cell is 3.0 units wide.
* `x` grows to the right, `y` grows **downward**; gravity is `+y`.
* Cell `(cx, cy)` spans `x ∈ [3cx, 3cx+3]`, `y ∈ [3cy, 3cy+3]`.
* Heading in degrees: `0` = up (against gravity), `90` = right, `180` = down,
  `270` = left. Positive rotation is clockwise on screen.
* Rope angle: `0` when the load hangs straight below the ship, positive when
  the load is to the right.

Default physics (`PhysicsParams`, stored in each set file so sets stay
reproducible when defaults change):

| Parameter | Value |
|-----------|-------|
| gravity | 2.0 u/s² |
| full thrust (ship alone) | 6.0 u/s² |
| load mass / ship mass | 0.3 (hover needs thrust 0.43) |
| ship radius / load radius | 0.5 / 0.15 |
| rope length | 0.7 |
| wall half thickness | 0.1 |
| rotation limit per turn | 45° |
| turn length / step | 0.5 s / 1/120 s |

## Agent interface (MCP)

Four tools. A run is bound to an agent name and a set.

| Tool | Input | Answer |
|------|-------|--------|
| `start_run` | `agent_name`, `set_name`, optional `model` | `run_id`, first observation |
| `observe` | `run_id` | current observation |
| `act` | `run_id`, `rotate_deg`, `thrust` (0–1), `note` (optional free text) | observation after the turn, event (`ok`, `wall`, `goal`, `turn_limit`) |
| `give_up` | `run_id` | next map or end of set |

The observation is text, not an image: the whole map as an ASCII grid with
coordinates, start and exit marked; position, velocity and heading of the
ship; position of the load, rope angle and angular velocity; turn and attempt
counters; the cell the ship is in. The agent sees everything; the difficulty
is in the flying, not in the exploring. The `note` is the place for the
agent's plan: it is logged and shown in the browser, it does not affect the
score.

```
Run 2f1c… | agent claude | set default | map 1 of 5 | score so far 0
Maze 8x8 cells, cell size 3.0 units. Start S at (1, 5), exit E at (5, 0) opening to the top border. @ = ship cell, o = load cell.
    0 1 2 3 4 5 6 7
  +--+--+--+--+--+  +--+--+
0 |           |   E       |
  +--+  +--+  +  +  +--+  +
  ...
Ship: pos (4.500, 16.500) in cell (1, 5), vel (0.000, 0.000) u/s, heading 0.0 deg (0=up, 90=right, 180=down, 270=left).
Load: pos (4.500, 17.200) in cell (1, 5), vel (0.000, 0.000) u/s. Rope: angle 0.0 deg (...), angular velocity 0.0 deg/s, taut.
Turn 0 of 300, attempt 1 of 5.
Physics: each turn simulates 0.50 s; gravity 2.00 u/s^2; ...
```

After a wall contact `act` answers with the start state of the next attempt.
After the goal it answers with the first observation of the next map. After
the fifth map the run ends and is scored. Every tool result also carries
`structuredContent` (JSON) with the same numbers plus, on `start_run` and at
every new map, the full wall geometry, so programmatic agents do not need to
parse text.

## Scoring

Points are awarded per map; the set is the sum. A map that was not completed
scores 0. The numbers are start values and live in the set file.

| Criterion | Points |
|-----------|--------|
| Map completed | 1000 |
| Each attempt after the first | −150 |
| Each turn in the successful attempt | −1 |
| Shortest path flown | +250 |

"Shortest path flown" means: the sequence of cells the ship passed through in
the successful attempt contains no cell that is not on some shortest BFS path.
With loops there can be several shortest paths; every one counts.

Every run stores set, agent name, model label, timestamps and all turns with
observation and note. Any run can be replayed in the browser. The leaderboard
shows per set the best run per model; because models vary between samples the
median over a model's runs is its value (marked provisional below three runs),
single runs stay visible. The reference pilot appears as model
`reference-pilot` and scores 5276 on `default`.

## Mazes and sets

Every maze is a function of seed and parameters: same seed, same map, on
every machine. The generator is a recursive backtracker (long corridors, few
branches, so much flying and little guessing) which yields a perfect maze
with exactly one path between any two cells. A loop parameter then opens
random interior walls so that the shortest path is not the only one. Start and
exit lie on opposite borders.

**Difficulty** is one number per map from measurable properties of the
shortest BFS path: length in cells, number of direction changes, number of
branches leading off the path, and the number of vertical moves upward
(against gravity, physically the hardest). The weights (`DifficultyWeights`)
are start values to be calibrated with real agent runs.

**Set builder.** `cold-trap-setbuilder build` tries seeds for each of the
five slots, scores every map, and flies it with the reference pilot. Seeds the
pilot cannot fly first try, or that need more than 85 % of the turn limit, are
rejected. From the remaining candidates it picks maps with rising difficulty
and writes the set file: name, generator version, physics and scoring
parameters, five seeds with their metrics. A set file is a benchmark artefact
that can be passed around.

| Map | Class | Grid | Loops |
|-----|-------|------|-------|
| 1 | easy | 8 × 8 | none |
| 2 | easy | 10 × 10 | none |
| 3 | medium | 14 × 14 | few (5 %) |
| 4 | medium | 18 × 18 | few (8 %) |
| 5 | hard | 24 × 24 | some (15 %) |

```bash
# build a new set and inspect it
cargo run --release -p cold-trap-setbuilder -- build --name mine --out sets/mine.json --seed-start 1000
cargo run --release -p cold-trap-setbuilder -- show sets/mine.json --fly
```

`GENERATOR_VERSION` in `crates/sim/src/lib.rs` is bumped whenever the mapping
from seed to maze changes; the server warns when a set was built with another
version.

## Web interface

The browser is a spectator seat, not a control. Left: the list of running
agents with name, model, set, maps 1 to 5, attempt, turn and points so far. A
click switches the main view to that agent; the others keep streaming in the
background. The main view draws the maze on a canvas: ship, rope, load, the
flight path of the current attempt, faint trails of failed attempts, the
shortest path on request. Every turn is played as a 0.5 s animation; if the
agent is faster than real time the playback speeds up to catch up. Right:
telemetry (position, velocity, heading, rope angle, last command, the agent's
note) and the scrolling list of turns. Finished runs can be replayed with
play, pause and step, map by map. The leaderboard page lists sets and models.
The operator can abort a run; there is no other intervention.

### HTTP API

| Route | Purpose |
|-------|---------|
| `POST /mcp` | MCP endpoint (streamable HTTP). |
| `GET /ws` | WebSocket stream of run events (`snapshot`, `run_started`, `turn`, `map_started`, `map_finished`, `run_finished`, `run_aborted`). |
| `GET /api/sets`, `GET /api/sets/{name}` | Loaded sets, with map geometry. |
| `GET /api/runs?limit=N` | Live and stored runs, newest first. |
| `GET /api/runs/{id}` | Run detail with maps and every turn (replay). |
| `POST /api/runs/{id}/abort` | Operator abort. |
| `GET /api/leaderboard` | Median and best score per set and model. |

## Technology

| Part | Choice |
|------|--------|
| Language | Rust (workspace of four crates) |
| MCP | [rmcp](https://crates.io/crates/rmcp) 3.9.0, the official Rust SDK, streamable HTTP transport |
| Web | axum 0.8 with WebSocket; browser in TypeScript without framework, Canvas 2D |
| Randomness | seedable PRNG (`rand_chacha`, ChaCha8), seed as `u64` |
| Storage | SQLite via `rusqlite` (bundled): sets, runs, turns |
| Set builder | own binary in the same workspace, uses simulation and generator |
| Reference pilot | scripted agent in Rust that flies the BFS path; drives the set builder and serves as baseline |

Determinism: the simulation runs only on the server, so there is no
cross-platform determinism question. The physics uses `f64` in a fixed
operation order; the same binary gives the same result for the same inputs.

## Development

```bash
cargo test --workspace --release        # unit tests incl. pilot success rates and a full engine run
cargo clippy --workspace --all-targets  # lint
cargo fmt --all                         # format
(cd web && npm run check)               # type-check the frontend
```

Useful ignored tests in `crates/pilot`: `trace_one` prints turn-by-turn traces
of crashing flights, `sweep` runs a parameter sweep of the pilot controller
(`cargo test -p cold-trap-pilot --release sweep -- --ignored --nocapture`).

## Decisions on the PRD's open questions

* **Rope:** a real rope that can go slack, solved as a distance constraint
  with an inelastic impulse. The slack case is handled naturally by only
  applying the constraint when the distance exceeds the rope length.
* **Turn length and rotation limit:** kept at 0.5 s and 45°. The reference
  pilot needs 130 to 260 turns per map on the default set, so 300 turns leaves
  headroom; the set builder rejects seeds that would push the pilot beyond
  85 % of the limit.
* **Whole map or field of view:** whole map. A field-of-view mode is not
  implemented yet.
* **Image of the map:** not provided; the observation is text only, so there
  is a single leaderboard.
* **Who starts agent apps:** the operator, by hand. The server only knows
  turns and never calls a model.
* **Name:** Cold Trap Maze remains the working title.

## Sources

* [Maze generation algorithm, Wikipedia](https://en.wikipedia.org/wiki/Maze_generation_algorithm)
* [MCP specification 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28)
* [rmcp on crates.io](https://crates.io/crates/rmcp), version 3.9.0
* [BALROG, Paglieri et al. 2024](https://arxiv.org/abs/2411.13543), model for a game benchmark with a public leaderboard

## License

MIT, see [LICENSE](LICENSE).
