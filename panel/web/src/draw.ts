// Shared drawing helpers for the two canvas scenes (the login scene and the map).

export const clamp = (x: number, a: number, b: number) => Math.max(a, Math.min(b, x));
export const lerp = (a: number, b: number, k: number) => a + (b - a) * k;
export const easeInOut = (x: number) => (x < 0.5 ? 4 * x * x * x : 1 - Math.pow(-2 * x + 2, 3) / 2);
export const smooth = (a: number, b: number, x: number) => {
  const k = clamp((x - a) / (b - a), 0, 1);
  return k * k * (3 - 2 * k);
};

/** Repeatable random numbers (a scene keeps its stars between frames and reloads). */
export function rng(seed: number): () => number {
  return () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646;
}

export interface Palette {
  night: boolean;
  bg: string;
  skyTop: string;
  s1: string;
  s2: string;
  earth: string;
  text: string;
  text2: string;
  text3: string;
  gold: string;
  water: string;
  waterBright: string;
  waterDeep: string;
  danger: string;
  warn: string;
  sprite: HTMLCanvasElement;
  redSprite: HTMLCanvasElement;
}

export function rgba(hex: string, a: number): string {
  const h = hex.replace("#", "");
  const n = parseInt(h.length === 3 ? h.replace(/./g, "$&$&") : h, 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`;
}

function glow(core: string, edge: string): HTMLCanvasElement {
  const c = document.createElement("canvas");
  c.width = c.height = 32;
  const g = c.getContext("2d")!;
  const grd = g.createRadialGradient(16, 16, 0, 16, 16, 16);
  grd.addColorStop(0, core);
  grd.addColorStop(0.35, rgba(edge, 0.55));
  grd.addColorStop(1, rgba(edge, 0));
  g.fillStyle = grd;
  g.fillRect(0, 0, 32, 32);
  return c;
}

/** The theme's colours, read from the CSS tokens. */
export function readPalette(): Palette {
  const cs = getComputedStyle(document.documentElement);
  const v = (n: string) => cs.getPropertyValue(n).trim();
  const night = document.documentElement.dataset.theme !== "dawn";
  const p = {
    night,
    bg: v("--bg"),
    skyTop: v("--sky-top"),
    s1: v("--stratum-1"),
    s2: v("--stratum-2"),
    earth: v("--earth"),
    text: v("--text"),
    text2: v("--text-2"),
    text3: v("--text-3"),
    gold: night ? "#e9c46a" : "#9a6a10",
    water: v("--water"),
    waterBright: v("--water-bright"),
    waterDeep: v("--water-deep"),
    danger: v("--danger"),
    warn: v("--warn"),
  };
  return { ...p, sprite: glow(p.waterBright, p.water), redSprite: glow("#ffb3b3", p.danger) };
}

/** Sizes a canvas to its box at the screen's pixel density (at most 2x). */
export function fitCanvas(canvas: HTMLCanvasElement) {
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const r = canvas.getBoundingClientRect();
  const w = Math.max(1, Math.round(r.width * dpr));
  const h = Math.max(1, Math.round(r.height * dpr));
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
  const ctx = canvas.getContext("2d")!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { ctx, W: r.width, H: r.height };
}

export const uiFont = (weight: number, px: number) =>
  `${weight} ${px}px "Space Grotesk", "Estedad", system-ui, sans-serif`;

export const quad = (p0: { x: number; y: number }, c: { x: number; y: number }, p1: { x: number; y: number }, k: number) => {
  const m = 1 - k;
  return { x: m * m * p0.x + 2 * m * k * c.x + k * k * p1.x, y: m * m * p0.y + 2 * m * k * c.y + k * k * p1.y };
};

let cached: { theme: string; palette: Palette } | null = null;

/** The palette of the current theme, read again only when the theme changes. */
export function paletteNow(): Palette {
  const theme = document.documentElement.dataset.theme ?? "night";
  if (!cached || cached.theme !== theme) cached = { theme, palette: readPalette() };
  return cached.palette;
}
