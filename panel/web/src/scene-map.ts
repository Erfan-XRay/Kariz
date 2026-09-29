// The map: servers are wells on the horizon, tunnels are channels of water between them,
// and the traffic is the particles in the water. Drawn on a canvas; the page around it
// (numbers, tooltip, text alternative) is React.
import { clamp, fitCanvas, lerp, quad, rgba, rng, uiFont } from "./draw";
import type { Palette } from "./draw";

export interface MapServer {
  id: string;
  name: string;
  /** Under the name: the role or place. */
  sub: string;
  st: "up" | "warn" | "down";
}

export interface MapTunnel {
  id: string;
  a: string;
  b: string;
  st: "up" | "down" | "off";
  /** Mbit/s, both directions together. */
  rate: number;
  /** The text on the channel. */
  label: string;
}

export interface MapData {
  servers: MapServer[];
  tunnels: MapTunnel[];
}

export interface MapHit {
  kind: "server" | "tunnel";
  id: string;
  x: number;
  y: number;
}

export interface MapOptions {
  data: () => MapData;
  palette: () => Palette;
  low: () => boolean;
  rtl: () => boolean;
  onHover: (hit: MapHit | null) => void;
  onOpen: (hit: MapHit) => void;
}

interface Particle {
  k: number;
  dir: number;
  s: number;
  j: number;
}

interface Geo {
  key: string;
  yH: number;
  narrow: boolean;
  pos: Record<string, number>;
  chans: { t: MapTunnel; pts: { x: number; y: number }[]; len: number }[];
  depth: Record<string, number>;
  stars: { x: number; y: number; z: number; ph: number }[];
  pebbles: { x: number; y: number; r: number }[];
}

export function startMap(canvas: HTMLCanvasElement, opts: MapOptions): () => void {
  let raf = 0;
  let stopped = false;
  let geo: Geo | null = null;
  let last = 0;
  let mouse: { x: number; y: number } | null = null;
  let hover: MapHit | null = null;
  const fill = new Map<string, number>();
  const parts = new Map<string, Particle[]>();

  const layout = (W: number, H: number): Geo => {
    const { servers, tunnels } = opts.data();
    const rtl = opts.rtl();
    const narrow = W < 640;
    const yH = narrow ? 110 : 150;
    const pad = narrow ? 44 : 110;
    const n = servers.length;
    const pos: Record<string, number> = {};
    servers.forEach((s, i) => {
      const x = n === 1 ? W / 2 : pad + (i * (W - 2 * pad)) / (n - 1);
      pos[s.id] = rtl ? W - x : x;
    });
    const base = yH + (narrow ? 52 : 64);
    const levels = Math.max(1, tunnels.length);
    const step = Math.min(narrow ? 32 : 38, (H - base - 44) / Math.max(1, levels - 1));
    const depth: Record<string, number> = {};
    const chans = tunnels
      .filter((t) => pos[t.a] !== undefined && pos[t.b] !== undefined)
      .map((t, level) => {
        const y0 = base + level * step;
        const p0 = { x: pos[t.a], y: y0 };
        const p1 = { x: pos[t.b], y: y0 + 16 };
        const c = { x: (p0.x + p1.x) / 2, y: y0 + 26 };
        const pts = Array.from({ length: 61 }, (_, i) => quad(p0, c, p1, i / 60));
        let len = 0;
        for (let i = 1; i < pts.length; i++) len += Math.hypot(pts[i].x - pts[i - 1].x, pts[i].y - pts[i - 1].y);
        depth[t.a] = Math.max(depth[t.a] ?? 0, p0.y);
        depth[t.b] = Math.max(depth[t.b] ?? 0, p1.y);
        return { t, pts, len };
      });
    const r = rng(11);
    return {
      key: key(W, H),
      yH,
      narrow,
      pos,
      chans,
      depth,
      stars: Array.from({ length: Math.round(W / 14) }, () => ({ x: r() * W, y: r() * (yH - 70) + 6, z: 0.3 + r() * 0.7, ph: r() * 6.28 })),
      pebbles: Array.from({ length: Math.round(W / 9) }, () => ({ x: r() * W, y: yH + 10 + r() * (H - yH - 14), r: 0.6 + r() * 1.4 })),
    };
  };

  const key = (W: number, H: number) => {
    const { servers, tunnels } = opts.data();
    return `${W}x${H}${opts.rtl()}${servers.map((s) => s.id).join()}|${tunnels.map((t) => t.id).join()}`;
  };

  const frame = (now: number) => {
    if (stopped) return;
    raf = requestAnimationFrame(frame);
    if (!canvas.isConnected || canvas.getBoundingClientRect().width === 0) return;
    const P = opts.palette();
    const { ctx, W, H } = fitCanvas(canvas);
    if (!geo || geo.key !== key(W, H)) geo = layout(W, H);
    const g = geo;
    const { servers } = opts.data();
    const time = now / 1000;
    const dt = Math.min(0.05, last ? time - last : 0.016);
    last = time;
    const still = opts.low();
    const rtl = opts.rtl();
    ctx.clearRect(0, 0, W, H);

    // Sky.
    const sky = ctx.createLinearGradient(0, 0, 0, g.yH);
    sky.addColorStop(0, P.bg);
    sky.addColorStop(1, P.s1);
    ctx.fillStyle = sky;
    ctx.fillRect(0, 0, W, g.yH);
    const gx = W * (rtl ? 0.25 : 0.75);
    const glow = ctx.createRadialGradient(gx, g.yH, 0, gx, g.yH, W * 0.5);
    glow.addColorStop(0, rgba(P.gold, P.night ? 0.12 : 0.22));
    glow.addColorStop(1, rgba(P.gold, 0));
    ctx.fillStyle = glow;
    ctx.fillRect(0, 0, W, g.yH);
    for (const s of g.stars) {
      const a = (P.night ? 0.7 : 0.2) * s.z * (still ? 0.8 : 0.6 + 0.4 * Math.sin(time * 1.3 + s.ph));
      ctx.fillStyle = P.night ? `rgba(233,240,255,${a})` : rgba(P.gold, a);
      ctx.fillRect(s.x, s.y, s.z * 1.6, s.z * 1.6);
    }

    // Earth.
    const earth = ctx.createLinearGradient(0, g.yH, 0, H);
    earth.addColorStop(0, P.earth);
    earth.addColorStop(1, P.night ? P.skyTop : P.s2);
    ctx.fillStyle = earth;
    ctx.fillRect(0, g.yH, W, H - g.yH);
    ctx.fillStyle = rgba(P.gold, P.night ? 0.07 : 0.12);
    for (const q of g.pebbles) {
      ctx.beginPath();
      ctx.arc(q.x, q.y, q.r, 0, 6.28);
      ctx.fill();
    }

    // Shafts.
    ctx.lineCap = "round";
    for (const s of servers) {
      ctx.strokeStyle = rgba(P.gold, 0.45);
      ctx.lineWidth = g.narrow ? 3 : 4;
      ctx.beginPath();
      ctx.moveTo(g.pos[s.id], g.yH);
      ctx.lineTo(g.pos[s.id], (g.depth[s.id] ?? g.yH + 40) + 4);
      ctx.stroke();
    }

    // Channels.
    for (const ch of g.chans) {
      const t = ch.t;
      const lit = hover?.kind === "tunnel" && hover.id === t.id;
      const target = t.st === "off" ? 0 : 1;
      const cur = fill.get(t.id) ?? 0;
      const speed = still ? 10 : 1.1;
      const now2 = cur < target ? Math.min(target, cur + dt * speed) : Math.max(target, cur - dt * speed);
      fill.set(t.id, now2);
      const reach = t.st === "down" ? 0.55 : 1;
      const path = (from: number, to: number) => {
        ctx.beginPath();
        for (let i = Math.floor(from * 60); i <= Math.ceil(to * 60); i++) {
          if (i === Math.floor(from * 60)) ctx.moveTo(ch.pts[i].x, ch.pts[i].y);
          else ctx.lineTo(ch.pts[i].x, ch.pts[i].y);
        }
      };
      ctx.lineWidth = g.narrow ? 12 : 16;
      ctx.strokeStyle = P.night ? "#06121f" : "#cdb489";
      path(0, 1);
      ctx.stroke();
      if (t.st === "off" || now2 < 1) {
        ctx.save();
        ctx.setLineDash([4, 6]);
        ctx.lineWidth = 1.2;
        ctx.strokeStyle = rgba(P.text3, lit ? 0.9 : 0.55);
        path(0, 1);
        ctx.stroke();
        ctx.restore();
      }
      const w = Math.min(now2, reach);
      if (w > 0.01) {
        const a = ch.pts[0];
        const b = ch.pts[60];
        const grad = ctx.createLinearGradient(a.x, 0, b.x, 0);
        grad.addColorStop(0, P.waterDeep);
        grad.addColorStop(1, lit ? P.waterBright : P.water);
        ctx.lineWidth = g.narrow ? 6 : 8;
        ctx.strokeStyle = grad;
        ctx.globalAlpha = still ? 0.35 + 0.65 * clamp(t.rate / 400, 0.2, 1) : 1;
        path(0, w);
        ctx.stroke();
        ctx.globalAlpha = 1;
      }
      if (t.st === "down" && now2 > 0.5) {
        const b = ch.pts[Math.round(reach * 60)];
        const k = still ? 0.5 : (time * 0.8) % 1;
        ctx.strokeStyle = rgba(P.danger, 1 - k);
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.arc(b.x, b.y, 6 + k * 16, 0, 6.28);
        ctx.stroke();
        ctx.fillStyle = P.danger;
        ctx.beginPath();
        ctx.arc(b.x, b.y, 4, 0, 6.28);
        ctx.fill();
        ctx.save();
        ctx.setLineDash([3, 5]);
        ctx.strokeStyle = rgba(P.danger, 0.5);
        ctx.lineWidth = 1.2;
        path(reach, 1);
        ctx.stroke();
        ctx.restore();
      }
      // Particles: downloads flow from the exit to the entry, uploads the other way.
      if (!still && t.st === "up") {
        const list = parts.get(t.id) ?? [];
        parts.set(t.id, list);
        const want = Math.round(clamp(6 + t.rate / 9, 6, 56));
        while (list.length < want) list.push({ k: Math.random(), dir: Math.random() < 0.8 ? -1 : 1, s: 0.5 + Math.random() * 0.7, j: Math.random() * 6 });
        list.length = Math.min(list.length, want);
        const v = clamp(50 + t.rate * 0.35, 50, 240) / ch.len;
        ctx.globalCompositeOperation = P.night ? "lighter" : "source-over";
        for (const q of list) {
          q.k += q.dir * v * dt * (q.dir < 0 ? 1 : 0.8);
          if (q.k < 0) q.k += 1;
          if (q.k > 1) q.k -= 1;
          if (q.k > now2) continue;
          const i = q.k * 60;
          const i0 = Math.floor(i);
          const f = i - i0;
          const a = ch.pts[i0];
          const b = ch.pts[Math.min(60, i0 + 1)];
          const x = lerp(a.x, b.x, f);
          const y = lerp(a.y, b.y, f) + Math.sin(time * 3 + q.j) * 1.5;
          const r = (q.dir < 0 ? 7 : 5) * q.s * (g.narrow ? 0.8 : 1);
          ctx.globalAlpha = q.dir < 0 ? 1 : 0.6;
          ctx.drawImage(P.sprite, x - r, y - r, r * 2, r * 2);
        }
        ctx.globalAlpha = 1;
        ctx.globalCompositeOperation = "source-over";
      }
    }

    // Channel labels.
    ctx.textBaseline = "middle";
    ctx.direction = rtl ? "rtl" : "ltr";
    g.chans.forEach((ch, i) => {
      const t = ch.t;
      const p = ch.pts[Math.round((0.28 + 0.14 * (i % 3)) * 60)];
      ctx.font = uiFont(500, g.narrow ? 10 : 12);
      const tw = ctx.measureText(t.label).width + 14;
      const y = p.y - (g.narrow ? 12 : 15);
      ctx.fillStyle = rgba(P.night ? P.skyTop : "#fbf6ec", 0.85);
      ctx.beginPath();
      ctx.roundRect(p.x - tw / 2, y - 9, tw, 18, 4);
      ctx.fill();
      ctx.strokeStyle = t.st === "down" ? rgba(P.danger, 0.6) : rgba(P.gold, hover?.id === t.id ? 0.7 : 0.25);
      ctx.lineWidth = 1;
      ctx.stroke();
      ctx.fillStyle = t.st === "down" ? P.danger : t.st === "off" ? P.text3 : P.text;
      ctx.textAlign = "center";
      ctx.fillText(t.label, p.x, y + 1);
    });

    // The horizon, the mounds and the servers' names.
    ctx.strokeStyle = P.gold;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(0, g.yH);
    ctx.lineTo(W, g.yH);
    ctx.stroke();
    servers.forEach((s, i) => {
      const x = g.pos[s.id];
      const lit = hover?.kind === "server" && hover.id === s.id;
      ctx.fillStyle = P.gold;
      ctx.beginPath();
      ctx.ellipse(x, g.yH, lit ? 20 : 16, lit ? 13 : 11, 0, Math.PI, 0);
      ctx.fill();
      const lift = g.narrow && i % 2 ? 26 : 0;
      ctx.textAlign = "center";
      ctx.font = uiFont(600, g.narrow ? 11 : 14);
      ctx.fillStyle = P.text;
      const ny = g.yH - (g.narrow ? 26 : 44) - lift;
      const nw = ctx.measureText(s.name).width;
      const lx = clamp(x, nw / 2 + 18, W - nw / 2 - 18);
      ctx.fillText(s.name, lx, ny);
      ctx.fillStyle = s.st === "up" ? P.water : s.st === "warn" ? P.warn : P.danger;
      ctx.beginPath();
      ctx.arc(lx - (nw / 2 + 8) * (rtl ? -1 : 1), ny, 3.5, 0, 6.28);
      ctx.fill();
      if (!g.narrow) {
        ctx.font = uiFont(400, 12);
        ctx.fillStyle = P.text3;
        ctx.fillText(s.sub, lx, ny + 18);
      }
    });

    if (mouse) {
      const h = hit(g, opts.data(), mouse.x, mouse.y);
      if (h?.id !== hover?.id || h?.kind !== hover?.kind) {
        hover = h;
        opts.onHover(h);
        canvas.style.cursor = h ? "pointer" : "default";
      } else if (h) opts.onHover(h);
    }
  };

  const hit = (g: Geo, data: MapData, x: number, y: number): MapHit | null => {
    for (const s of data.servers) {
      if (Math.abs(x - g.pos[s.id]) < 40 && y > g.yH - 70 && y < g.yH + 8) return { kind: "server", id: s.id, x, y };
    }
    let best: MapHit | null = null;
    let bd = 14;
    for (const ch of g.chans) {
      for (const p of ch.pts) {
        const d = Math.hypot(p.x - x, p.y - y);
        if (d < bd) {
          bd = d;
          best = { kind: "tunnel", id: ch.t.id, x, y };
        }
      }
    }
    return best;
  };

  const onMove = (e: PointerEvent) => {
    const r = canvas.getBoundingClientRect();
    mouse = { x: e.clientX - r.left, y: e.clientY - r.top };
  };
  const onLeave = () => {
    mouse = null;
    if (hover) opts.onHover(null);
    hover = null;
    canvas.style.cursor = "default";
  };
  const onClick = () => {
    if (hover) opts.onOpen(hover);
  };
  canvas.addEventListener("pointermove", onMove);
  canvas.addEventListener("pointerleave", onLeave);
  canvas.addEventListener("click", onClick);
  raf = requestAnimationFrame(frame);

  return () => {
    stopped = true;
    cancelAnimationFrame(raf);
    canvas.removeEventListener("pointermove", onMove);
    canvas.removeEventListener("pointerleave", onLeave);
    canvas.removeEventListener("click", onClick);
  };
}
