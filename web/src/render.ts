// Canvas renderer for one map: walls, markers, trails, ship, rope, load.

import type { Frame, MapView, Vec2 } from "./types.js";

export interface Trail {
  points: Vec2[];
  failed: boolean;
}

export interface RenderOptions {
  showShortest: boolean;
  showTrails: boolean;
  thrust: number;
}

const COLORS = {
  background: "#0f1218",
  wall: "#d7dbe3",
  grid: "rgba(255,255,255,0.04)",
  start: "rgba(80, 200, 120, 0.35)",
  exit: "rgba(255, 196, 70, 0.4)",
  shortest: "rgba(90, 150, 255, 0.22)",
  trail: "rgba(120, 200, 255, 0.9)",
  trailFailed: "rgba(255, 110, 110, 0.35)",
  ship: "#e8f0ff",
  shipOutline: "#5aa0ff",
  heading: "#ffd166",
  flame: "rgba(255, 140, 40, 0.9)",
  rope: "#c9d1e0",
  load: "#ff9f5a",
};

export class Renderer {
  private ctx: CanvasRenderingContext2D;
  private scale = 1;
  private offset: Vec2 = { x: 0, y: 0 };

  constructor(private canvas: HTMLCanvasElement) {
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("canvas 2d context unavailable");
    this.ctx = ctx;
  }

  /** Fit the maze plus a margin for the exit into the canvas. */
  private fit(map: MapView): void {
    const dpr = window.devicePixelRatio || 1;
    const rect = this.canvas.getBoundingClientRect();
    const w = Math.max(1, Math.floor(rect.width * dpr));
    const h = Math.max(1, Math.floor(rect.height * dpr));
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
    const margin = map.cell_size * 1.2;
    const worldW = map.width * map.cell_size + 2 * margin;
    const worldH = map.height * map.cell_size + 2 * margin;
    this.scale = Math.min(w / worldW, h / worldH);
    this.offset = {
      x: (w - (map.width * map.cell_size) * this.scale) / 2,
      y: (h - (map.height * map.cell_size) * this.scale) / 2,
    };
  }

  private px(p: Vec2): Vec2 {
    return { x: this.offset.x + p.x * this.scale, y: this.offset.y + p.y * this.scale };
  }

  draw(map: MapView, frame: Frame | null, trails: Trail[], opts: RenderOptions): void {
    this.fit(map);
    const ctx = this.ctx;
    const cs = map.cell_size;
    ctx.fillStyle = COLORS.background;
    ctx.fillRect(0, 0, this.canvas.width, this.canvas.height);

    // Cell markers.
    const fillCell = (c: { x: number; y: number }, color: string) => {
      const a = this.px({ x: c.x * cs, y: c.y * cs });
      ctx.fillStyle = color;
      ctx.fillRect(a.x, a.y, cs * this.scale, cs * this.scale);
    };
    if (opts.showShortest) {
      for (const c of map.shortest_cells) fillCell(c, COLORS.shortest);
    }
    fillCell(map.start, COLORS.start);
    fillCell(map.exit, COLORS.exit);

    // Exit arrow.
    {
      const c = { x: (map.exit.x + 0.5) * cs, y: (map.exit.y + 0.5) * cs };
      const dir = sideVector(map.exit_side);
      const a = this.px({ x: c.x + dir.x * cs * 0.55, y: c.y + dir.y * cs * 0.55 });
      const b = this.px({ x: c.x + dir.x * cs * 1.1, y: c.y + dir.y * cs * 1.1 });
      ctx.strokeStyle = "rgba(255,196,70,0.8)";
      ctx.lineWidth = Math.max(1, 0.08 * cs * this.scale);
      ctx.beginPath();
      ctx.moveTo(a.x, a.y);
      ctx.lineTo(b.x, b.y);
      ctx.stroke();
    }

    // Walls.
    ctx.strokeStyle = COLORS.wall;
    ctx.lineCap = "round";
    ctx.lineWidth = Math.max(1.5, 2 * map.physics.wall_half_thickness * this.scale);
    ctx.beginPath();
    for (let y = 0; y <= map.height; y++) {
      for (let x = 0; x < map.width; x++) {
        if (map.h_walls[y * map.width + x]) {
          const a = this.px({ x: x * cs, y: y * cs });
          const b = this.px({ x: (x + 1) * cs, y: y * cs });
          ctx.moveTo(a.x, a.y);
          ctx.lineTo(b.x, b.y);
        }
      }
    }
    for (let y = 0; y < map.height; y++) {
      for (let x = 0; x <= map.width; x++) {
        if (map.v_walls[y * (map.width + 1) + x]) {
          const a = this.px({ x: x * cs, y: y * cs });
          const b = this.px({ x: x * cs, y: (y + 1) * cs });
          ctx.moveTo(a.x, a.y);
          ctx.lineTo(b.x, b.y);
        }
      }
    }
    ctx.stroke();

    // Trails.
    if (opts.showTrails) {
      for (const trail of trails) {
        if (trail.points.length < 2) continue;
        ctx.strokeStyle = trail.failed ? COLORS.trailFailed : COLORS.trail;
        ctx.lineWidth = trail.failed ? 1 : 1.5;
        ctx.beginPath();
        const first = this.px(trail.points[0]);
        ctx.moveTo(first.x, first.y);
        for (let i = 1; i < trail.points.length; i++) {
          const p = this.px(trail.points[i]);
          ctx.lineTo(p.x, p.y);
        }
        ctx.stroke();
        if (trail.failed) {
          const last = this.px(trail.points[trail.points.length - 1]);
          ctx.strokeStyle = "rgba(255,110,110,0.8)";
          ctx.lineWidth = 1.5;
          const r = 0.25 * cs * this.scale * 0.5;
          ctx.beginPath();
          ctx.moveTo(last.x - r, last.y - r);
          ctx.lineTo(last.x + r, last.y + r);
          ctx.moveTo(last.x + r, last.y - r);
          ctx.lineTo(last.x - r, last.y + r);
          ctx.stroke();
        }
      }
    }

    if (!frame) return;
    const ship = this.px(frame.ship);
    const load = this.px(frame.load);
    const rShip = map.physics.ship_radius * this.scale;
    const rLoad = map.physics.load_radius * this.scale;

    // Rope.
    ctx.strokeStyle = COLORS.rope;
    ctx.lineWidth = Math.max(1, 0.04 * this.scale);
    ctx.beginPath();
    ctx.moveTo(ship.x, ship.y);
    ctx.lineTo(load.x, load.y);
    ctx.stroke();

    // Load.
    ctx.fillStyle = COLORS.load;
    ctx.beginPath();
    ctx.arc(load.x, load.y, rLoad, 0, Math.PI * 2);
    ctx.fill();

    // Flame (opposite to heading, length by thrust).
    const h = (frame.heading_deg * Math.PI) / 180;
    const dir = { x: Math.sin(h), y: -Math.cos(h) };
    if (opts.thrust > 0.02) {
      const len = rShip * (0.6 + 1.6 * opts.thrust);
      const base = { x: ship.x - dir.x * rShip * 0.9, y: ship.y - dir.y * rShip * 0.9 };
      const tip = { x: ship.x - dir.x * (rShip * 0.9 + len), y: ship.y - dir.y * (rShip * 0.9 + len) };
      const n = { x: -dir.y, y: dir.x };
      ctx.fillStyle = COLORS.flame;
      ctx.beginPath();
      ctx.moveTo(base.x + n.x * rShip * 0.35, base.y + n.y * rShip * 0.35);
      ctx.lineTo(tip.x, tip.y);
      ctx.lineTo(base.x - n.x * rShip * 0.35, base.y - n.y * rShip * 0.35);
      ctx.closePath();
      ctx.fill();
    }

    // Ship body and heading tick.
    ctx.fillStyle = COLORS.ship;
    ctx.strokeStyle = COLORS.shipOutline;
    ctx.lineWidth = Math.max(1, 0.06 * this.scale);
    ctx.beginPath();
    ctx.arc(ship.x, ship.y, rShip, 0, Math.PI * 2);
    ctx.fill();
    ctx.stroke();
    ctx.strokeStyle = COLORS.heading;
    ctx.lineWidth = Math.max(1.5, 0.08 * this.scale);
    ctx.beginPath();
    ctx.moveTo(ship.x, ship.y);
    ctx.lineTo(ship.x + dir.x * rShip * 1.4, ship.y + dir.y * rShip * 1.4);
    ctx.stroke();
  }
}

export function sideVector(side: string): Vec2 {
  switch (side) {
    case "top":
      return { x: 0, y: -1 };
    case "bottom":
      return { x: 0, y: 1 };
    case "left":
      return { x: -1, y: 0 };
    default:
      return { x: 1, y: 0 };
  }
}

/** Interpolate the frame at time `t` within a frame list (sorted by t). */
export function frameAt(frames: Frame[], t: number): Frame | null {
  if (frames.length === 0) return null;
  if (t <= frames[0].t) return frames[0];
  const last = frames[frames.length - 1];
  if (t >= last.t) return last;
  let lo = 0;
  let hi = frames.length - 1;
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1;
    if (frames[mid].t <= t) lo = mid;
    else hi = mid;
  }
  const a = frames[lo];
  const b = frames[hi];
  const span = b.t - a.t;
  const u = span > 0 ? (t - a.t) / span : 0;
  let dh = b.heading_deg - a.heading_deg;
  if (dh > 180) dh -= 360;
  if (dh < -180) dh += 360;
  return {
    t,
    ship: { x: a.ship.x + (b.ship.x - a.ship.x) * u, y: a.ship.y + (b.ship.y - a.ship.y) * u },
    load: { x: a.load.x + (b.load.x - a.load.x) * u, y: a.load.y + (b.load.y - a.load.y) * u },
    heading_deg: a.heading_deg + dh * u,
  };
}
