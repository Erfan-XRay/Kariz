// The hero of the front page: a journey down a qanat. It opens on the desert at night (the
// panel's login scene, from the same tokens), then, as the page scrolls, rides the bucket down
// the mother well past the strata of the earth, turns into the channel where the water (the
// traffic) flows, and follows it to where it splits into streams, each carrying its packets to
// a target. The last stretch moves one to one with the scroll, down into rock that has become
// the page's own background, so the first section follows with no seam.
//
// The world is drawn left to right, upstream to downstream, and mirrored for right to left.
// What does not move (the strata, their texture, the shafts, the tunnel) is drawn once into an
// offscreen canvas; each frame blits it and draws the light, the water and the small things.
// It runs only while it is on screen, and holds still under reduced motion or low power.
import { clamp, fitCanvas, lerp, paletteNow, rgba, rng, smooth } from "./panel/draw";
import type { Palette } from "./panel/draw";

const TAU = Math.PI * 2;
const root = document.documentElement;
const reduced = matchMedia("(prefers-reduced-motion: reduce)");
const isStill = () => root.dataset.low === "1" || reduced.matches;

/** An ease with no jolt at either end, over the part of the scroll between a and b. */
const ease = (x: number, a: number, b: number) => 0.5 - 0.5 * Math.cos(Math.PI * clamp((x - a) / (b - a), 0, 1));

function hex(c: string): [number, number, number] {
  const h = c.replace("#", "");
  const n = parseInt(h.length === 3 ? h.replace(/./g, "$&$&") : h, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
/** A colour between a and b (both #rrggbb). */
function mix(a: string, b: string, k: number): string {
  const x = hex(a);
  const y = hex(b);
  return `rgb(${x.map((v, i) => Math.round(lerp(v, y[i], k))).join(",")})`;
}

function sprite(core: string, edge: string, size = 64): HTMLCanvasElement {
  const c = document.createElement("canvas");
  c.width = c.height = size;
  const g = c.getContext("2d")!;
  const r = size / 2;
  const grd = g.createRadialGradient(r, r, 0, r, r, r);
  grd.addColorStop(0, core);
  grd.addColorStop(0.3, rgba(edge, 0.5));
  grd.addColorStop(1, rgba(edge, 0));
  g.fillStyle = grd;
  g.fillRect(0, 0, size, size);
  return c;
}

/** The hero's own colours for each theme, around the tokens. */
interface Look {
  mount: [string, string];
  dune: [string, string];
  strata: [string, string][];
  rock: string;
  hole: string;
  wall: string;
  mound: string;
  hair: string;
  root: string;
  speck: string;
  glow: string;
  core: string;
  lit: string;
  shade: string;
  bucket: string;
  rim: string;
  light: GlobalCompositeOperation;
}

function lookFor(P: Palette): Look {
  if (P.night)
    return {
      mount: ["#0c1c33", "#0f223c"],
      dune: ["#122746", "#0e2139"],
      strata: [
        ["#14253d", "#112138"],
        ["#191f39", "#161c34"],
        ["#0f1c31", "#0d192c"],
        ["#0a2133", "#0a1e2f"],
      ],
      rock: "#0a182a",
      hole: "#030912",
      wall: "#07111f",
      mound: "#18304f",
      hair: P.gold,
      root: rgba(P.gold, 0.13),
      speck: "#9fb7d6",
      glow: P.waterBright,
      core: P.water,
      lit: "#e9fffb",
      shade: "#02060c",
      bucket: "#3b2c12",
      rim: P.gold,
      light: "lighter",
    };
  return {
    mount: ["#eadcc0", "#e2cfab"],
    dune: ["#e8cfa0", "#dfc28c"],
    strata: [
      ["#d7bc8b", "#cfb27f"],
      ["#c99f79", "#c09470"],
      ["#cbb38c", "#c2a882"],
      ["#b5bea5", "#a9b59c"],
    ],
    rock: "#d5c6a8",
    hole: "#4a3928",
    wall: "#5d4832",
    mound: "#cfa86c",
    hair: P.gold,
    root: "rgba(92,60,20,.22)",
    speck: "#5b4526",
    glow: "#35c7b8",
    core: "#169c8f",
    lit: "#f2fffd",
    shade: "#3a2a15",
    bucket: "#7a4e00",
    rim: "#e2b865",
    light: "source-over",
  };
}

interface Star {
  x: number;
  y: number;
  z: number;
  ph: number;
  sp: number;
  gold: boolean;
}
interface Stream {
  xs: Float32Array;
  ys: Float32Array;
  len: Float32Array;
  total: number;
  path: Path2D;
  end: { x: number; y: number };
  hit: number;
}
interface Label {
  y: number;
  depth: string;
  name: string;
}

/** Everything that depends only on the size of the view and the theme. */
interface World {
  key: string;
  W: number;
  H: number;
  u: number;
  L: Look;
  cam0: { x: number; y: number };
  panX: number;
  G: number;
  th: number;
  yWater: number;
  yFloor: number;
  xStart: number;
  xSplit: number;
  xEnd: number;
  wells: { x: number; w: number; h: number }[];
  axleY: number;
  bound: ((x: number) => number)[];
  hairs: Path2D;
  shafts: Path2D;
  ruler: Path2D;
  tunnel: Path2D;
  mounds: Path2D;
  mountains: Path2D[];
  dunes: Path2D[];
  ground: { c: HTMLCanvasElement; x0: number; k: number };
  milky: HTMLCanvasElement | null;
  moon: HTMLCanvasElement;
  dust: HTMLCanvasElement;
  spark: HTMLCanvasElement;
  warm: HTMLCanvasElement;
  stars: Star[];
  motes: { x: number; y: number; z: number; ph: number }[];
  drops: { x: number; v: number; k: number; s: number }[];
  streams: Stream[];
  packets: { s: number; t: number; v: number }[];
  labels: Label[];
  font: string;
  grads: {
    band: CanvasGradient;
    water: CanvasGradient;
    ceiling: CanvasGradient;
    moonShaft: CanvasGradient;
    upShaft: CanvasGradient;
    vignette: CanvasGradient;
  };
}

// The strata, and the depths the sections below the hero are named after.
const NAMES = {
  en: ["mother well", "soil", "clay", "gravel", "aquifer", "channel"],
  fa: ["مادرچاه", "خاک", "رس", "شن", "آبخوان", "مجرا"],
};
const DEPTHS = [0, 4, 12, 25, 40, 60];

function build(ctx: CanvasRenderingContext2D, W: number, H: number, dpr: number, P: Palette, fa: boolean): World {
  const L = lookFor(P);
  const u = H;
  const r = rng(11);
  const s = clamp(W * 0.15, 84, 190); // between the wells
  const G = 2.3 * u; // the middle of the channel
  const th = clamp(0.13 * u, 64, 118); // the channel's height
  const yWater = G + th * 0.06;
  const yFloor = G + th / 2 - 3;
  const xStart = -0.55 * s;
  const xSplit = 4.8 * s;
  const reach = clamp(0.42 * W, 150, 560);
  const xEnd = xSplit + reach;
  const panX = xSplit + 0.2 * W;
  const cam0 = { x: Math.min(2 * s, 0.32 * W), y: -0.12 * u };
  const yLast = G - 0.02 * u + 0.29 * u;

  // Wells: the mother well upstream, the others shallower in look only (they all reach the channel).
  const w0 = clamp(0.055 * W, 26, 44);
  const wells = [0, 1, 2, 3, 4].map((i) => ({ x: i * s, w: i ? w0 * (0.5 - i * 0.03) : w0, h: w0 * (i ? 0.34 - i * 0.03 : 0.5) }));
  const axleY = -wells[0].h - w0 * 1.15;

  // Strata boundaries, gently wavy.
  const bases = [0.5 * u, 1.05 * u, 1.65 * u, G + th * 0.22];
  const bound = bases.map((b, i) => {
    const amp = i === 3 ? 0.008 * u : 0.022 * u;
    const f1 = 0.0035 + r() * 0.002;
    const f2 = 0.011 + r() * 0.006;
    const p1 = r() * TAU;
    const p2 = r() * TAU;
    return (x: number) => b + amp * (0.65 * Math.sin(x * f1 + p1) + 0.35 * Math.sin(x * f2 + p2));
  });

  const gx0 = Math.min(cam0.x, 0) - W / 2 - 80;
  const gx1 = panX + W / 2 + 80;
  const gy1 = yLast + H / 2 + 60;

  const hairs = new Path2D();
  for (const f of bound) {
    for (let x = gx0; x <= gx1; x += 8) (x === gx0 ? hairs.moveTo.bind(hairs) : hairs.lineTo.bind(hairs))(x, f(x));
  }
  const shaftBottom = G - th / 2 + 1;
  const shafts = new Path2D();
  for (const wl of wells) {
    shafts.moveTo(wl.x - wl.w / 2, 0);
    shafts.lineTo(wl.x - wl.w / 2, shaftBottom);
    shafts.moveTo(wl.x + wl.w / 2, 0);
    shafts.lineTo(wl.x + wl.w / 2, shaftBottom);
  }
  const ruler = new Path2D();
  const rx = -w0 / 2 - 5;
  for (let i = 0, y = 0; y < shaftBottom - 6; i++, y += u * 0.025) {
    const long = i % 4 === 0;
    ruler.moveTo(rx, y);
    ruler.lineTo(rx - (long ? 9 : 4), y);
  }

  const tunnel = new Path2D();
  const tEnd = xSplit + th * 0.3;
  const rr = th / 2;
  tunnel.moveTo(xStart + rr, G - rr);
  tunnel.lineTo(tEnd - rr, G - rr);
  tunnel.arc(tEnd - rr, G, rr, -Math.PI / 2, Math.PI / 2);
  tunnel.lineTo(xStart + rr, G + rr);
  tunnel.arc(xStart + rr, G, rr, Math.PI / 2, Math.PI * 1.5);
  tunnel.closePath();

  const mounds = new Path2D();
  for (const wl of wells) {
    // A ring of spoil around the mouth, cut through: two soft humps.
    const R = w0 * (wl.x ? 1.25 : 1.9);
    for (const sd of [-1, 1]) {
      const a = wl.x + (sd * wl.w) / 2;
      mounds.moveTo(a + sd * R, 0.5);
      mounds.bezierCurveTo(a + sd * R * 0.55, 0.5, a + sd * R * 0.4, -wl.h * 1.15, a + sd * R * 0.12, -wl.h);
      mounds.quadraticCurveTo(a + sd * 1.5, -wl.h * 0.9, a, -wl.h * 0.55);
      mounds.lineTo(a, 0.5);
      mounds.closePath();
    }
  }

  // Mountains upstream (where the water comes from) and dunes all along.
  const ridge = (x0: number, x1: number, peak: number, seed: number, step: number) => {
    const q = rng(seed);
    const p = new Path2D();
    p.moveTo(x0, 40);
    let y = 0;
    for (let x = x0; x <= x1; x += step) {
      const env = Math.sin(clamp((x - x0) / (x1 - x0), 0, 1) * Math.PI);
      y = lerp(y, -peak * env * (0.55 + q() * 0.45), 0.55);
      p.lineTo(x, y);
    }
    p.lineTo(x1, 40);
    p.closePath();
    return p;
  };
  const mx0 = cam0.x - W / 2 - 160;
  const mountains = [ridge(mx0, 1.7 * s, 0.13 * u, 3, 26), ridge(mx0 + 40, 0.9 * s, 0.085 * u, 9, 18)];
  const dune = (amp: number, base: number, freq: number, off: number) => {
    const p = new Path2D();
    const x0 = cam0.x - W;
    const x1 = cam0.x + W;
    p.moveTo(x0, 60);
    for (let x = x0; x <= x1; x += 10)
      p.lineTo(x, Math.min(0, -base - amp * (0.6 * Math.sin(x * freq + off) + 0.4 * Math.sin(x * freq * 2.3 + off * 1.7))));
    p.lineTo(x1, 60);
    p.closePath();
    return p;
  };
  const S = clamp(W / 1000, 0.55, 1.15);
  const dunes = [dune(20 * S, 16 * S, 0.004, 1.2), dune(13 * S, 3 * S, 0.007, 3.1)];

  // Streams: after the channel, the water splits, one stream per connection, to its target.
  const N = 4;
  const gap = clamp(0.055 * u, 22, 46);
  const sx0 = xSplit - th * 0.15;
  const sy0 = (yWater + yFloor) / 2;
  const streams: Stream[] = [];
  for (let i = 0; i < N; i++) {
    const ty = G + 0.03 * u + (i - (N - 1) / 2) * gap;
    const bx = xSplit + reach * 0.55;
    const n1 = 48;
    const n2 = 16;
    const xs = new Float32Array(n1 + n2 + 1);
    const ys = new Float32Array(n1 + n2 + 1);
    for (let j = 0; j <= n1; j++) {
      const t = j / n1;
      const m = 1 - t;
      const c1x = sx0 + (bx - sx0) * 0.45;
      const c2x = sx0 + (bx - sx0) * 0.5;
      xs[j] = m * m * m * sx0 + 3 * m * m * t * c1x + 3 * m * t * t * c2x + t * t * t * bx;
      ys[j] = m * m * m * sy0 + 3 * m * m * t * sy0 + 3 * m * t * t * ty + t * t * t * ty;
    }
    for (let j = 1; j <= n2; j++) {
      xs[n1 + j] = lerp(bx, xEnd, j / n2);
      ys[n1 + j] = ty;
    }
    const len = new Float32Array(xs.length);
    const path = new Path2D();
    path.moveTo(xs[0], ys[0]);
    for (let j = 1; j < xs.length; j++) {
      len[j] = len[j - 1] + Math.hypot(xs[j] - xs[j - 1], ys[j] - ys[j - 1]);
      path.lineTo(xs[j], ys[j]);
    }
    streams.push({ xs, ys, len, total: len[len.length - 1], path, end: { x: xEnd, y: ty }, hit: -9 });
  }
  const packets = Array.from({ length: N * 3 }, (_, i) => ({ s: i % N, t: r(), v: 0.07 + r() * 0.06 }));

  const stars = Array.from({ length: Math.round((W * H * 0.62) / 3600) }, () => ({
    x: r(),
    y: Math.pow(r(), 1.3),
    z: 0.3 + r() * 0.7,
    ph: r() * TAU,
    sp: 0.6 + r() * 1.8,
    gold: r() < 0.16,
  }));
  const motes = Array.from({ length: 34 }, () => ({ x: r(), y: r(), z: Math.pow(r(), 1.6) * 1.6 + 0.2, ph: r() * TAU }));
  const drops = Array.from({ length: Math.round(clamp((xSplit - xStart) / 22, 24, 60)) }, () => ({
    x: lerp(xStart, xSplit, r()),
    v: 30 + r() * 40,
    k: r(),
    s: 0.5 + r() * 0.7,
  }));

  const nm = NAMES[fa ? "fa" : "en"];
  const digits = (x: number) => (fa ? String(x).replace(/\d/g, (d) => "۰۱۲۳۴۵۶۷۸۹"[+d]) : String(x));
  const ys = [0.07 * u, 0.3 * u, 0.8 * u, 1.36 * u, 1.93 * u, G + th / 2 + 0.05 * u];
  const labels = ys.map((y, i) => ({ y, depth: `${DEPTHS[i] ? "−" : ""}${digits(DEPTHS[i])} m`, name: nm[i] }));
  const font = getComputedStyle(root).getPropertyValue("--font-mono").trim() || "monospace";

  // Gradients in world space (the transform applies when they are used).
  const band = ctx.createLinearGradient(0, G - 0.9 * u, 0, G + 0.5 * u);
  band.addColorStop(0, rgba(L.glow, 0));
  band.addColorStop(0.55, rgba(L.glow, P.night ? 0.1 : 0.07));
  band.addColorStop(0.7, rgba(L.glow, P.night ? 0.12 : 0.06));
  band.addColorStop(1, rgba(L.glow, 0));
  const water = ctx.createLinearGradient(0, yWater, 0, yFloor);
  water.addColorStop(0, rgba(L.glow, P.night ? 0.95 : 0.9));
  water.addColorStop(0.35, rgba(L.core, 0.9));
  water.addColorStop(1, rgba(P.waterDeep, 0.95));
  const ceiling = ctx.createLinearGradient(0, G - th / 2, 0, yWater);
  ceiling.addColorStop(0, rgba(L.glow, 0.03));
  ceiling.addColorStop(1, rgba(L.glow, P.night ? 0.3 : 0.22));
  const moonShaft = ctx.createLinearGradient(0, 0, 0, 0.8 * u);
  moonShaft.addColorStop(0, P.night ? "rgba(200,220,255,.14)" : "rgba(255,248,230,.14)");
  moonShaft.addColorStop(1, "rgba(200,220,255,0)");
  const upShaft = ctx.createLinearGradient(0, shaftBottom, 0, shaftBottom - 0.8 * u);
  upShaft.addColorStop(0, rgba(L.glow, P.night ? 0.3 : 0.22));
  upShaft.addColorStop(0.4, rgba(L.glow, P.night ? 0.08 : 0.05));
  upShaft.addColorStop(1, rgba(L.glow, 0));
  const vignette = ctx.createRadialGradient(W / 2, H / 2, Math.min(W, H) * 0.3, W / 2, H / 2, Math.hypot(W, H) * 0.62);
  vignette.addColorStop(0, rgba(P.night ? P.skyTop : "#5a4630", 0));
  vignette.addColorStop(1, rgba(P.night ? P.skyTop : "#5a4630", P.night ? 0.75 : 0.3));

  const world: World = {
    key: "",
    W,
    H,
    u,
    L,
    cam0,
    panX,
    G,
    th,
    yWater,
    yFloor,
    xStart,
    xSplit,
    xEnd,
    wells,
    axleY,
    bound,
    hairs,
    shafts,
    ruler,
    tunnel,
    mounds,
    mountains,
    dunes,
    ground: { c: document.createElement("canvas"), x0: gx0, k: 1 },
    milky: P.night ? milkyWay(W, H, r) : null,
    moon: moon(P),
    spark: sprite(L.lit, L.glow, 48),
    dust: sprite(P.night ? "rgba(255,246,220,.9)" : "rgba(255,255,255,.9)", P.night ? "#f4d58a" : "#ffffff", 32),
    warm: sprite(rgba(P.gold, 0.5), P.gold, 64),
    stars,
    motes,
    drops,
    streams,
    packets,
    labels,
    font,
    grads: { band, water, ceiling, moonShaft, upShaft, vignette },
  };
  paintGround(world, P, gx0, gx1, gy1, dpr, r);
  return world;
}

/** The still part of the underground: strata with their texture, the shafts and the channel. */
function paintGround(w: World, P: Palette, gx0: number, gx1: number, gy1: number, dpr: number, r: () => number) {
  const { L, u, G, th, bound } = w;
  const ww = gx1 - gx0;
  const k = Math.min(dpr, Math.sqrt(7.5e6 / (ww * gy1)));
  const c = w.ground.c;
  c.width = Math.ceil(ww * k);
  c.height = Math.ceil(gy1 * k);
  w.ground.k = k;
  const g = c.getContext("2d")!;
  g.setTransform(k, 0, 0, k, -gx0 * k, 0);
  const between = (top: ((x: number) => number) | null, bot: ((x: number) => number) | null) => {
    const p = new Path2D();
    p.moveTo(gx0, top ? top(gx0) : 0);
    for (let x = gx0; x <= gx1 + 8; x += 8) p.lineTo(x, top ? top(x) : 0);
    for (let x = gx1 + 8; x >= gx0; x -= 8) p.lineTo(x, bot ? bot(x) : gy1);
    p.closePath();
    return p;
  };
  const randIn = (top: ((x: number) => number) | null, bot: ((x: number) => number) | null) => {
    const x = gx0 + r() * ww;
    const a = top ? top(x) : 0;
    const b = bot ? bot(x) : gy1;
    return { x, y: a + r() * (b - a), a, b };
  };

  // Bands.
  const tops = [null, ...bound];
  for (let i = 0; i < 5; i++) {
    const top = tops[i];
    const bot = i < 4 ? bound[i] : null;
    const y0 = i === 0 ? 0 : bound[i - 1](0);
    const y1 = i < 4 ? bound[i](0) : gy1;
    const grd = g.createLinearGradient(0, y0, 0, i < 4 ? y1 : G + 0.5 * u);
    if (i < 4) {
      grd.addColorStop(0, L.strata[i][0]);
      grd.addColorStop(1, L.strata[i][1]);
    } else {
      grd.addColorStop(0, L.rock);
      grd.addColorStop(0.35, mix(L.rock, P.bg, 0.55));
      grd.addColorStop(1, P.bg);
    }
    const path = between(top, bot);
    g.fillStyle = grd;
    g.fill(path);
    g.save();
    g.clip(path);
    const area = ww * Math.max(1, y1 - y0);
    // Soft variations in tone, so no band is a flat colour.
    if (i < 4)
      for (let n = ww / 140; n > 0; n--) {
        const q = randIn(top, bot);
        const rad = 30 + r() * 110;
        const tone = r() < 0.5 ? L.speck : L.shade;
        const gr = g.createRadialGradient(q.x, q.y, 0, q.x, q.y, rad);
        gr.addColorStop(0, rgba(tone, 0.035 + r() * 0.04));
        gr.addColorStop(1, rgba(tone, 0));
        g.fillStyle = gr;
        g.fillRect(q.x - rad, q.y - rad, rad * 2, rad * 2);
      }
    if (i === 0) {
      // Soil: grains, a few stones, and roots from the surface.
      for (let n = area / 260; n > 0; n--) {
        const q = randIn(top, bot);
        g.fillStyle = rgba(r() < 0.5 ? L.speck : L.shade, 0.05 + r() * 0.12);
        g.fillRect(q.x, q.y, 0.6 + r() * 1.3, 0.6 + r() * 1.3);
      }
      for (let n = area / 22000; n > 0; n--) pebble(g, randIn(top, bot), 2 + r() * 5, L, i, r);
      g.lineCap = "round";
      const branch = (x: number, y: number, a: number, len: number, lw: number, d: number) => {
        const x1 = x + Math.cos(a) * len;
        const y1 = y + Math.sin(a) * len;
        g.strokeStyle = L.root;
        g.lineWidth = lw;
        g.beginPath();
        g.moveTo(x, y);
        g.quadraticCurveTo(x + Math.cos(a + 0.5) * len * 0.5, y + Math.sin(a + 0.5) * len * 0.5, x1, y1);
        g.stroke();
        if (d < 4)
          for (let b = 0; b < 2; b++)
            if (r() < 0.8) branch(x1, y1, a + (r() - 0.5) * 1.1, len * (0.55 + r() * 0.2), lw * 0.62, d + 1);
      };
      for (let x = gx0; x < gx1; x += 50 + r() * 90) branch(x, 0, Math.PI / 2 + (r() - 0.5) * 0.7, 0.04 * u + r() * 0.05 * u, 1.8, 0);
    } else if (i === 1) {
      // Clay: fine laminations that follow the band.
      const layers = Math.round((y1 - y0) / 5);
      for (let j = 0; j < layers; j++) {
        const t = (j + 0.5) / layers;
        const wob = r() * TAU;
        g.strokeStyle = rgba(r() < 0.5 ? L.speck : L.shade, 0.04 + r() * 0.08);
        g.lineWidth = 0.6 + r() * 1.4;
        g.beginPath();
        for (let x = gx0; x <= gx1; x += 12) {
          const y = lerp(top!(x), bot!(x), t) + Math.sin(x * 0.02 + wob) * 1.2;
          if (x === gx0) g.moveTo(x, y);
          else g.lineTo(x, y);
        }
        g.stroke();
      }
      for (let n = area / 30000; n > 0; n--) pebble(g, randIn(top, bot), 1.5 + r() * 3, L, i, r);
    } else if (i === 2) {
      // Gravel: pebbles, each lit from above.
      for (let n = area / 520; n > 0; n--) pebble(g, randIn(top, bot), 1.5 + Math.pow(r(), 2) * 8, L, i, r);
    } else if (i === 3) {
      // The aquifer: wet sand and gravel, with water glinting in it.
      for (let n = area / 1500; n > 0; n--) pebble(g, randIn(top, bot), 1.2 + Math.pow(r(), 2) * 4, L, i, r);
      for (let n = area / 380; n > 0; n--) {
        const q = randIn(top, bot);
        g.fillStyle = rgba(L.glow, 0.06 + r() * 0.3 * ((q.y - q.a) / (q.b - q.a + 1)));
        g.fillRect(q.x, q.y, 0.7 + r() * 1.2, 0.7 + r() * 1.2);
      }
      for (let n = ww / 70; n > 0; n--) {
        const q = randIn(top, bot);
        g.strokeStyle = rgba(L.glow, 0.08 + r() * 0.1);
        g.lineWidth = 1;
        g.beginPath();
        g.moveTo(q.x, q.y);
        g.lineTo(q.x + 10 + r() * 40, q.y + (r() - 0.5) * 2);
        g.stroke();
      }
    } else {
      // Rock: a few cracks near the channel, fading into the page.
      for (let n = ww / 120; n > 0; n--) {
        let x = gx0 + r() * ww;
        let y = bound[3](x) + r() * 0.3 * u;
        g.strokeStyle = rgba(L.shade, 0.25 * (1 - (y - G) / (0.5 * u)));
        g.lineWidth = 1;
        g.beginPath();
        g.moveTo(x, y);
        for (let j = 0; j < 5; j++) {
          x += 6 + r() * 14;
          y += (r() - 0.4) * 10;
          g.lineTo(x, y);
        }
        g.stroke();
      }
    }
    g.restore();
  }
  // A soft shadow under each boundary and a faint light on top of it: the strata have depth.
  for (const f of bound.slice(0, 3)) {
    const p = new Path2D();
    for (let x = gx0; x <= gx1; x += 8) (x === gx0 ? p.moveTo.bind(p) : p.lineTo.bind(p))(x, f(x));
    g.save();
    g.translate(0, 6);
    g.strokeStyle = rgba(L.shade, P.night ? 0.3 : 0.12);
    g.lineWidth = 12;
    g.stroke(p);
    g.restore();
  }

  // Shafts: dark, with a lit back wall, a stone collar and footholds.
  for (const wl of w.wells) {
    const x0 = wl.x - wl.w / 2;
    const yb = G - th / 2 + 1;
    const grd = g.createLinearGradient(x0, 0, x0 + wl.w, 0);
    grd.addColorStop(0, L.hole);
    grd.addColorStop(0.3, L.wall);
    grd.addColorStop(0.7, L.wall);
    grd.addColorStop(1, L.hole);
    g.fillStyle = grd;
    g.fillRect(x0, 0, wl.w, yb);
    for (let y = 0; y < 0.1 * u; y += 7 + r() * 4) {
      const hgt = 5 + r() * 3;
      for (const side of [-1, 1]) {
        const sx = side < 0 ? x0 - 5 : x0 + wl.w;
        g.fillStyle = mix(L.strata[0][0], P.night ? "#3a4a66" : "#f3e3c2", 0.35 + r() * 0.3);
        g.fillRect(sx, y + 0.5, 5, hgt - 1);
      }
    }
    for (let y = 0.12 * u, side = 0; y < yb - 10; y += 18, side ^= 1) {
      g.fillStyle = rgba(L.shade, 0.7);
      g.fillRect(side ? x0 + wl.w - 4 : x0 + 1, y, 3, 4);
    }
  }

  // The channel: a vaulted gallery lined with hoops of fired clay.
  const grd = g.createLinearGradient(0, G - th / 2, 0, G + th / 2);
  grd.addColorStop(0, L.hole);
  grd.addColorStop(0.45, L.wall);
  grd.addColorStop(1, L.hole);
  g.fillStyle = grd;
  g.fill(w.tunnel);
  g.save();
  g.clip(w.tunnel);
  g.strokeStyle = rgba(P.gold, P.night ? 0.12 : 0.2);
  g.lineWidth = 1.2;
  for (let x = w.xStart + th * 0.5; x < w.xSplit; x += th * 0.62) {
    if (w.wells.some((wl) => Math.abs(wl.x - x) < wl.w * 0.7)) continue;
    g.beginPath();
    g.ellipse(x, G, th * 0.1, th / 2 - 1, 0, 0, TAU);
    g.stroke();
  }
  g.restore();
}

function pebble(g: CanvasRenderingContext2D, q: { x: number; y: number }, rx: number, L: Look, band: number, r: () => number) {
  const base = L.strata[Math.min(band, 3)][0];
  const ry = rx * (0.55 + r() * 0.3);
  const a = (r() - 0.5) * 0.8;
  g.fillStyle = mix(base, r() < 0.35 ? L.speck : L.shade, 0.1 + r() * 0.22);
  g.beginPath();
  g.ellipse(q.x, q.y, rx, ry, a, 0, TAU);
  g.fill();
  g.strokeStyle = rgba(L.speck, 0.12 + r() * 0.12);
  g.lineWidth = 0.8;
  g.beginPath();
  g.ellipse(q.x, q.y, rx * 0.8, ry * 0.7, a, Math.PI * 1.1, Math.PI * 1.7);
  g.stroke();
}

function milkyWay(W: number, H: number, r: () => number): HTMLCanvasElement {
  const c = document.createElement("canvas");
  const h = Math.round(H * 0.7);
  c.width = Math.round(W);
  c.height = h;
  const g = c.getContext("2d")!;
  // A soft band across the sky, low on one side and high on the other.
  const at = (t: number) => ({ x: t * W, y: h * (0.95 - 0.8 * t) });
  for (let i = 0; i < 14; i++) {
    const t = (i + r()) / 14;
    const p = at(t);
    const rad = (0.08 + r() * 0.1) * Math.max(W, H);
    const gr = g.createRadialGradient(p.x, p.y, 0, p.x, p.y, rad);
    gr.addColorStop(0, `rgba(170,190,255,${0.03 + r() * 0.03})`);
    gr.addColorStop(1, "rgba(170,190,255,0)");
    g.fillStyle = gr;
    g.fillRect(p.x - rad, p.y - rad, rad * 2, rad * 2);
  }
  for (let i = 0; i < (W * H) / 900; i++) {
    const t = r();
    const p = at(t);
    const d = (r() + r() + r() - 1.5) * 0.09 * Math.max(W, H);
    g.fillStyle = `rgba(225,232,255,${0.08 + r() * 0.3})`;
    const sz = r() < 0.9 ? 0.7 : 1.2;
    g.fillRect(p.x + d * 0.6, p.y + d, sz, sz);
  }
  return c;
}

function moon(P: Palette): HTMLCanvasElement {
  const c = document.createElement("canvas");
  c.width = c.height = 160;
  const g = c.getContext("2d")!;
  const gr = g.createRadialGradient(80, 80, 10, 80, 80, 80);
  gr.addColorStop(0, rgba(P.night ? "#fff3d0" : "#ffffff", P.night ? 0.22 : 0.5));
  gr.addColorStop(1, rgba(P.night ? "#fff3d0" : "#ffffff", 0));
  g.fillStyle = gr;
  g.fillRect(0, 0, 160, 160);
  if (P.night) {
    const d = document.createElement("canvas");
    d.width = d.height = 160;
    const h = d.getContext("2d")!;
    h.fillStyle = "#fbefcf";
    h.beginPath();
    h.arc(80, 80, 17, 0, TAU);
    h.fill();
    h.globalCompositeOperation = "destination-out";
    h.beginPath();
    h.arc(89, 74, 16, 0, TAU);
    h.fill();
    g.drawImage(d, 0, 0);
  }
  return c;
}

export function startHero(canvas: HTMLCanvasElement, section: HTMLElement, fade: HTMLElement[]) {
  let raf = 0;
  let visible = true;
  let mx = 0.5;
  let my = 0.5;
  let w: World | null = null;
  let last = 0;
  let shoot: { x: number; y: number; t0: number } | null = null;

  addEventListener(
    "pointermove",
    (e) => {
      mx = e.clientX / innerWidth;
      my = e.clientY / innerHeight;
    },
    { passive: true },
  );

  /** How far along the journey the reader is, as a fraction and in pixels of scroll. */
  const progress = () => {
    const r = section.getBoundingClientRect();
    const span = r.height - innerHeight;
    const px = clamp(-r.top, 0, Math.max(0, span));
    return { p: span > 0 ? px / span : 0, px, span };
  };

  const camera = (w: World, pr: { p: number; px: number; span: number }) => {
    const { p, px, span } = pr;
    const u = w.u;
    const x = lerp(w.cam0.x, 0, ease(p, 0.02, 0.34)) + w.panX * ease(p, 0.5, 0.84);
    let y = lerp(w.cam0.y, w.G - 0.02 * u, ease(p, 0.03, 0.62));
    // The last stretch moves one to one with the page, so letting go of the view is seamless.
    const Lt = 0.4 * u;
    const R = 0.22 * u;
    const q = span > 0 ? clamp(px - (span - Lt), 0, Lt) : 0;
    const k = q / R;
    y += q < R ? R * (k * k * k - (k * k * k * k) / 2) : q - R / 2;
    return { x, y };
  };

  const draw = (now: number) => {
    const P = paletteNow();
    const { ctx, W, H } = fitCanvas(canvas);
    const dpr = canvas.width / W;
    const fa = root.lang === "fa";
    const key = `${W}x${H}|${root.dataset.theme}|${fa}|${dpr}`;
    if (!w || w.key !== key) {
      w = build(ctx, W, H, dpr, P, fa);
      w.key = key;
    }
    const time = now / 1000;
    const dt = clamp(time - last, 0, 0.05);
    last = time;
    const still = isStill();
    const rtl = root.dir === "rtl";
    const m = rtl ? -1 : 1;
    const { L, u, G, th } = w;
    const pr = still ? { p: 0, px: 0, span: 0 } : progress();
    const p = pr.p;
    const cam = camera(w, pr);
    const dy = cam.y - w.cam0.y;

    // The words above the horizon rise a little and fade as the camera goes down.
    const t = 1 - smooth(0.015, 0.14, p);
    for (const el of fade) {
      el.style.opacity = String(t);
      el.toggleAttribute("inert", t < 0.4);
    }
    fade[0].style.transform = dy > 0.5 ? `translate3d(0,${(-dy * 0.35).toFixed(1)}px,0)` : "";

    /** Draws in world coordinates, for a layer that moves f times as fast as the ground. */
    const layer = (f = 1) => {
      const cx = w!.cam0.x + (cam.x - w!.cam0.x) * f;
      const cy = w!.cam0.y + (cam.y - w!.cam0.y) * f;
      ctx.setTransform(m * dpr, 0, 0, dpr, dpr * (W / 2 - m * cx), dpr * (H / 2 - cy));
    };
    const screen = () => ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const sxOf = (x: number) => W / 2 + m * (x - cam.x);
    const syOf = (y: number) => H / 2 + (y - cam.y);
    const vx0 = cam.x - W / 2;
    const vx1 = cam.x + W / 2;
    const vy0 = cam.y - H / 2;
    const vy1 = cam.y + H / 2;
    const under = smooth(0.1 * u, 0.8 * u, cam.y); // how far below the surface the view is
    const end = smooth(0.84, 1, p); // the hand-over to the page

    screen();
    ctx.globalCompositeOperation = "source-over";
    ctx.fillStyle = P.bg;
    ctx.fillRect(0, 0, W, H);

    // ---- The sky ----
    const hy = syOf(0);
    if (hy > 0) {
      const lift = dy * 0.3;
      const sky = ctx.createLinearGradient(0, -lift, 0, hy);
      sky.addColorStop(0, P.skyTop);
      sky.addColorStop(0.75, P.bg);
      sky.addColorStop(1, P.night ? mix(P.bg, "#1d3a62", 0.45) : mix(P.bg, "#f1d9a8", 0.6));
      ctx.fillStyle = sky;
      ctx.fillRect(0, 0, W, hy);
      const glow = ctx.createRadialGradient(W / 2, hy, 0, W / 2, hy, W * 0.65);
      glow.addColorStop(0, rgba(P.gold, P.night ? 0.13 : 0.3));
      glow.addColorStop(1, rgba(P.gold, 0));
      ctx.fillStyle = glow;
      ctx.fillRect(0, 0, W, hy);
      const px = still ? 0 : (mx - 0.5) * 14;
      const py = still ? 0 : (my - 0.5) * 8;
      if (w.milky) {
        ctx.globalAlpha = 0.9;
        ctx.save();
        if (rtl) ctx.setTransform(-dpr, 0, 0, dpr, W * dpr, 0);
        ctx.drawImage(w.milky, px * 0.3, -lift * 0.8 + py * 0.3, W, w.milky.height);
        ctx.restore();
        ctx.globalAlpha = 1;
      }
      ctx.globalCompositeOperation = P.night ? "lighter" : "source-over";
      for (const s of w.stars) {
        const y = s.y * H * 0.6 - lift * (0.6 + s.z * 0.4) + py * s.z;
        if (y > hy - 8 || y < -4) continue;
        const tw = still ? 0.8 : 0.55 + 0.45 * Math.sin(time * s.sp + s.ph);
        const a = (P.night ? 0.9 : 0.3) * s.z * tw * (1 - (y / hy) * 0.55);
        ctx.fillStyle = s.gold ? rgba(P.gold, a) : P.night ? `rgba(233,240,255,${a})` : rgba(P.gold, a * 0.6);
        const sz = s.z * 2.2;
        ctx.fillRect(s.x * W + px * s.z - sz / 2, y - sz / 2, sz, sz);
      }
      ctx.globalCompositeOperation = "source-over";
      const mX = rtl ? W * 0.18 : W * 0.82;
      const mY = H * 0.17 - lift * 0.7;
      if (P.night) ctx.drawImage(w.moon, mX - 80 + px * 0.5, mY - 80 + py * 0.5, 160, 160);
      else ctx.drawImage(w.moon, W / 2 - 240, hy - 240 * 0.62, 480, 480 * 0.62);

      if (P.night && !still && p < 0.1) {
        if (!shoot && Math.random() < 0.003) shoot = { x: Math.random() * W * 0.7, y: Math.random() * hy * 0.35, t0: time };
        if (shoot) {
          const k = (time - shoot.t0) / 0.9;
          if (k > 1) shoot = null;
          else {
            const x = shoot.x + k * 260;
            const y = shoot.y + k * 90 - lift;
            const g = ctx.createLinearGradient(x - 90, y - 31, x, y);
            g.addColorStop(0, "rgba(255,255,255,0)");
            g.addColorStop(1, `rgba(255,248,225,${0.8 * (1 - k)})`);
            ctx.strokeStyle = g;
            ctx.lineWidth = 1.4;
            ctx.beginPath();
            ctx.moveTo(x - 90, y - 31);
            ctx.lineTo(x, y);
            ctx.stroke();
          }
        }
      }

      layer(0.35);
      ctx.fillStyle = L.mount[0];
      ctx.fill(w.mountains[0]);
      layer(0.5);
      ctx.fillStyle = L.mount[1];
      ctx.fill(w.mountains[1]);
      layer(0.7);
      ctx.fillStyle = rgba(L.dune[0], P.night ? 0.85 : 0.9);
      ctx.fill(w.dunes[0]);
      layer(0.85);
      ctx.fillStyle = L.dune[1];
      ctx.fill(w.dunes[1]);
    }

    // ---- The ground ----
    layer(1);
    if (vy1 > 0) {
      const gd = w.ground;
      const k = gd.k;
      const sx = Math.max(0, (vx0 - gd.x0) * k);
      const sy = Math.max(0, vy0 * k);
      const sw = Math.min(gd.c.width - sx, W * k + 2);
      const sh = Math.min(gd.c.height - sy, H * k + 2);
      if (sw > 0 && sh > 0) ctx.drawImage(gd.c, sx, sy, sw, sh, gd.x0 + sx / k, sy / k, sw / k, sh / k);
    }

    // Hairlines: the strata, the shafts, the channel, the depth ruler.
    ctx.lineWidth = 1;
    ctx.strokeStyle = rgba(L.hair, P.night ? 0.26 : 0.32);
    ctx.stroke(w.hairs);
    ctx.strokeStyle = rgba(L.hair, P.night ? 0.2 : 0.3);
    ctx.stroke(w.shafts);
    ctx.strokeStyle = rgba(L.hair, P.night ? 0.3 : 0.4);
    ctx.stroke(w.ruler);
    ctx.strokeStyle = rgba(L.hair, P.night ? 0.42 : 0.5);
    ctx.lineWidth = 1.2;
    ctx.stroke(w.tunnel);

    // The surface: spoil mounds around the wells, the windlass over the mother well.
    if (vy0 < 20) {
      ctx.fillStyle = L.mound;
      ctx.fill(w.mounds);
      ctx.strokeStyle = rgba(P.gold, 0.55);
      ctx.lineWidth = 1;
      ctx.stroke(w.mounds);
      ctx.strokeStyle = P.gold;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.moveTo(vx0 - 10, 0);
      ctx.lineTo(vx1 + 10, 0);
      ctx.stroke();
      const w0 = w.wells[0];
      ctx.strokeStyle = rgba(P.gold, 0.9);
      ctx.lineWidth = 2;
      ctx.lineCap = "round";
      ctx.beginPath();
      for (const sd of [-1, 1]) {
        ctx.moveTo(sd * w0.w * 0.95, -w0.h * 0.7);
        ctx.lineTo(sd * w0.w * 0.62, w.axleY - 6);
        ctx.moveTo(sd * w0.w * 0.3, -w0.h * 0.95);
        ctx.lineTo(sd * w0.w * 0.62, w.axleY - 6);
      }
      ctx.moveTo(-w0.w * 0.75, w.axleY);
      ctx.lineTo(w0.w * 0.85, w.axleY);
      ctx.lineTo(w0.w * 0.85, w.axleY + 7);
      ctx.stroke();
      ctx.lineCap = "butt";
    }

    // ---- Light ----
    ctx.globalCompositeOperation = L.light;
    ctx.fillStyle = w.grads.band;
    ctx.fillRect(vx0, G - 0.9 * u, W, 1.4 * u);
    ctx.fillStyle = w.grads.moonShaft;
    for (const wl of w.wells.slice(1)) ctx.fillRect(wl.x - wl.w / 2, 0, wl.w, 0.8 * u);
    ctx.fillStyle = w.grads.upShaft;
    const w0 = w.wells[0];
    ctx.fillRect(w0.x - w0.w / 2, G - th / 2 - 0.8 * u, w0.w, 0.8 * u);
    ctx.globalCompositeOperation = "source-over";

    // ---- The rope and the bucket, riding down with the reader ----
    const bucketH = w0.w * 0.36;
    const by = clamp(cam.y + 0.05 * u, w.axleY + w0.w * 0.6, w.yWater - bucketH * 0.55);
    const ropeLen = by - w.axleY;
    const sway = still ? 0 : Math.sin(time * 1.1) * Math.min(w0.w * 0.12, ropeLen * 0.004);
    ctx.strokeStyle = rgba(L.rim, 0.8);
    ctx.lineWidth = 1.2;
    ctx.beginPath();
    ctx.moveTo(0, w.axleY);
    ctx.quadraticCurveTo(sway * 0.3, (w.axleY + by) / 2, sway, by);
    ctx.stroke();
    if (vy1 > by - 200 && vy0 < by + 200) {
      const bw = w0.w * 0.42;
      if (!still) {
        ctx.globalCompositeOperation = L.light;
        const lr = 0.22 * u;
        ctx.globalAlpha = P.night ? 0.45 * under : 0.25 * under;
        ctx.drawImage(w.warm, sway - lr, by - lr, lr * 2, lr * 2);
        ctx.globalAlpha = 1;
        ctx.globalCompositeOperation = "source-over";
      }
      ctx.fillStyle = L.bucket;
      ctx.strokeStyle = L.rim;
      ctx.lineWidth = 1.2;
      ctx.beginPath();
      ctx.moveTo(sway - bw / 2, by);
      ctx.lineTo(sway + bw / 2, by);
      ctx.lineTo(sway + bw * 0.38, by + bucketH);
      ctx.lineTo(sway - bw * 0.38, by + bucketH);
      ctx.closePath();
      ctx.fill();
      ctx.stroke();
      ctx.beginPath();
      ctx.arc(sway, by, bw / 2, Math.PI, TAU);
      ctx.stroke();
    }

    // ---- The water ----
    const tx0 = Math.max(w.xStart, vx0 - 10);
    const tx1 = Math.min(w.xSplit + th * 0.3, vx1 + 10);
    const waterOn = vy1 > G - th && vy0 < G + th && tx1 > tx0;
    if (waterOn) {
      ctx.save();
      ctx.clip(w.tunnel);
      ctx.globalCompositeOperation = L.light;
      ctx.fillStyle = w.grads.ceiling;
      ctx.fillRect(tx0, G - th / 2, tx1 - tx0, w.yWater - (G - th / 2));
      ctx.globalCompositeOperation = "source-over";
      ctx.fillStyle = w.grads.water;
      ctx.fillRect(tx0, w.yWater, tx1 - tx0, w.yFloor - w.yWater + 4);
      // The surface, rippling downstream.
      const wave = (x: number) =>
        w!.yWater + Math.sin(x * 0.045 - time * 2.6) * 1.1 + Math.sin(x * 0.012 + time * 0.9) * 1.4;
      ctx.beginPath();
      for (let x = tx0; x <= tx1; x += 6) {
        if (x === tx0) ctx.moveTo(x, wave(x));
        else ctx.lineTo(x, wave(x));
      }
      ctx.globalCompositeOperation = L.light;
      ctx.strokeStyle = rgba(L.glow, 0.35);
      ctx.lineWidth = 6;
      ctx.stroke();
      ctx.strokeStyle = rgba(L.lit, 0.9);
      ctx.lineWidth = 1.3;
      ctx.stroke();
      // Currents inside the water.
      ctx.lineWidth = 1;
      const depth = w.yFloor - w.yWater;
      for (let j = 0; j < 3; j++) {
        const y = w.yWater + depth * (0.28 + j * 0.24);
        ctx.setLineDash([6 + j * 10, 22 + j * 14]);
        ctx.lineDashOffset = still ? j * 9 : -time * (46 - j * 9);
        ctx.strokeStyle = rgba(L.lit, 0.32 - j * 0.07);
        ctx.beginPath();
        ctx.moveTo(tx0, y);
        ctx.lineTo(tx1, y);
        ctx.stroke();
      }
      ctx.setLineDash([]);
      // Light where the moon comes down the other wells.
      for (const wl of w.wells.slice(1)) {
        const rr = wl.w * 1.3;
        ctx.globalAlpha = P.night ? 0.5 : 0.35;
        ctx.drawImage(w.dust, wl.x - rr, w.yWater - rr * 0.4, rr * 2, rr * 0.8);
      }
      ctx.globalAlpha = 1;
      // Grains of light carried by the water.
      for (const d of w.drops) {
        if (!still) {
          d.x += d.v * dt;
          if (d.x > w.xSplit) d.x = w.xStart + th * 0.4;
        }
        if (d.x < tx0 - 20 || d.x > tx1 + 20) continue;
        const y = w.yWater + 3 + d.k * (depth - 6) + Math.sin(time * 2 + d.x * 0.05) * 1.2;
        const rr = 5 * d.s;
        ctx.drawImage(w.spark, d.x - rr, y - rr, rr * 2, rr * 2);
      }
      ctx.globalCompositeOperation = "source-over";
      ctx.restore();
    }

    // The streams: the traffic split per connection, each to its target.
    const sx0 = w.xSplit - th * 0.3;
    if (vx1 > sx0 && vy1 > G - 0.3 * u && vy0 < G + 0.4 * u) {
      ctx.globalCompositeOperation = L.light;
      ctx.lineCap = "round";
      for (const st of w.streams) {
        ctx.strokeStyle = rgba(L.glow, P.night ? 0.16 : 0.14);
        ctx.lineWidth = 9;
        ctx.stroke(st.path);
        ctx.strokeStyle = rgba(L.glow, 0.85);
        ctx.lineWidth = 1.8;
        ctx.stroke(st.path);
        ctx.setLineDash([2, 16]);
        ctx.lineDashOffset = still ? 0 : -time * 40;
        ctx.strokeStyle = rgba(L.lit, 0.7);
        ctx.lineWidth = 1.2;
        ctx.stroke(st.path);
        ctx.setLineDash([]);
      }
      for (const pk of w.packets) {
        const st = w.streams[pk.s];
        if (!still) {
          pk.t += (pk.v * dt * 520) / st.total;
          if (pk.t >= 1) {
            pk.t -= 1;
            st.hit = time;
          }
        }
        const d = pk.t * st.total;
        let j = 1;
        while (j < st.len.length - 1 && st.len[j] < d) j++;
        const k = (d - st.len[j - 1]) / (st.len[j] - st.len[j - 1] || 1);
        const x = lerp(st.xs[j - 1], st.xs[j], k);
        const y = lerp(st.ys[j - 1], st.ys[j], k);
        const a = Math.atan2(st.ys[j] - st.ys[j - 1], st.xs[j] - st.xs[j - 1]);
        const fadeIn = smooth(0, 0.08, pk.t) * (1 - smooth(0.94, 1, pk.t));
        ctx.globalAlpha = fadeIn;
        ctx.drawImage(w.spark, x - 14, y - 14, 28, 28);
        ctx.save();
        ctx.translate(x, y);
        ctx.rotate(a);
        ctx.fillStyle = L.lit;
        ctx.beginPath();
        ctx.roundRect(-6, -2, 12, 4, 2);
        ctx.fill();
        ctx.restore();
      }
      ctx.globalAlpha = 1;
      ctx.lineCap = "butt";
      ctx.globalCompositeOperation = "source-over";
      for (const st of w.streams) {
        const { x, y } = st.end;
        const pulse = still ? 0 : Math.exp(-(time - st.hit) * 3);
        if (pulse > 0.01) {
          ctx.globalCompositeOperation = L.light;
          ctx.globalAlpha = pulse;
          ctx.drawImage(w.spark, x + 8 - 26, y - 26, 52, 52);
          ctx.globalAlpha = 1;
          ctx.globalCompositeOperation = "source-over";
        }
        ctx.strokeStyle = P.gold;
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        ctx.moveTo(x - 9, y - 5);
        ctx.lineTo(x - 3, y);
        ctx.lineTo(x - 9, y + 5);
        ctx.stroke();
        ctx.fillStyle = L.hole;
        ctx.beginPath();
        ctx.arc(x + 8, y, 5, 0, TAU);
        ctx.fill();
        ctx.stroke();
      }
    }

    // ---- Depth of field: motes of dust, the near ones large and soft ----
    screen();
    if (under > 0.01) {
      ctx.globalCompositeOperation = L.light;
      for (const d of w.motes) {
        const f = 0.5 + d.z;
        const drift = still ? 0 : time * 6 * d.z;
        const X = ((((d.x * (W + 80) - m * (cam.x - w.cam0.x) * f * 0.6 + drift) % (W + 80)) + W + 80) % (W + 80)) - 40;
        const Y = ((((d.y * (H + 80) - dy * f - drift * 0.4) % (H + 80)) + H + 80) % (H + 80)) - 40;
        const sz = 2 + d.z * d.z * 7;
        const tw = 0.6 + 0.4 * Math.sin(time * 0.8 + d.ph);
        ctx.globalAlpha = under * (1 - end) * tw * (d.z > 1.1 ? 0.18 : 0.5) * (P.night ? 1 : 0.7);
        ctx.drawImage(d.z > 1.1 ? w.spark : w.dust, X - sz, Y - sz, sz * 2, sz * 2);
      }
      ctx.globalAlpha = 1;
      ctx.globalCompositeOperation = "source-over";
      ctx.globalAlpha = under * (1 - end);
      ctx.fillStyle = w.grads.vignette;
      ctx.fillRect(0, 0, W, H);
      ctx.globalAlpha = 1;
    }

    // ---- Depth marks by the mother well ----
    ctx.textBaseline = "middle";
    ctx.direction = rtl ? "rtl" : "ltr";
    ctx.textAlign = rtl ? "right" : "left";
    const fs = W < 600 ? 11 : 12;
    for (const lb of w.labels) {
      const y = syOf(lb.y);
      if (y < 40 || y > H - 10) continue;
      const a = smooth(56, 120, y) * (1 - smooth(H - 70, H - 20, y)) * (1 - end);
      if (a < 0.02) continue;
      const x0 = sxOf(w0.w / 2 + 6);
      ctx.globalAlpha = a;
      ctx.strokeStyle = rgba(P.gold, 0.6);
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(x0, y);
      ctx.lineTo(x0 + m * 18, y);
      ctx.stroke();
      ctx.font = `500 ${fs}px ${w.font}`;
      ctx.fillStyle = P.night ? P.gold : "#5e3c00";
      const tx = x0 + m * 24;
      ctx.fillText(lb.depth, tx, y);
      const dw = ctx.measureText(lb.depth).width;
      ctx.fillStyle = P.text2;
      ctx.fillText(lb.name, tx + m * (dw + 14), y);
    }
    ctx.globalAlpha = 1;

    // Held still, the hero is one screen: its foot melts into the page.
    if (still) {
      const f = ctx.createLinearGradient(0, H * 0.8, 0, H);
      f.addColorStop(0, rgba(P.bg, 0));
      f.addColorStop(1, P.bg);
      ctx.fillStyle = f;
      ctx.fillRect(0, H * 0.8, W, H * 0.2);
    }
  };

  const loop = (now: number) => {
    raf = 0;
    if (!visible || document.hidden) return;
    draw(now);
    if (!isStill()) raf = requestAnimationFrame(loop);
  };
  const kick = () => {
    if (!raf && visible) raf = requestAnimationFrame(loop);
  };

  new IntersectionObserver((es) => {
    visible = es[0].isIntersecting;
    kick();
  }).observe(section);
  document.addEventListener("visibilitychange", kick);
  // Held still: draw again when something that changes the picture changes.
  const redraw = () => {
    if (isStill()) requestAnimationFrame(() => draw(performance.now()));
    else kick();
  };
  addEventListener("resize", redraw);
  addEventListener("scroll", redraw, { passive: true });
  new MutationObserver(redraw).observe(root, { attributes: true, attributeFilter: ["data-theme", "data-low"] });
  reduced.addEventListener("change", redraw);
  section.classList.add("ready");
  kick();
}
