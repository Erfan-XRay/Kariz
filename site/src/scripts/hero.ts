// The hero of the front page: the qanat at night (the panel's login scene, drawn the same
// way from the same tokens), with the camera moving down the well as the page scrolls.
// It runs only while it is on screen, and holds still under reduced motion or low power.
import { clamp, easeInOut, fitCanvas, lerp, paletteNow, rgba, rng, smooth } from "./panel/draw";

interface Star {
  x: number;
  y: number;
  z: number;
  ph: number;
  sp: number;
  gold: boolean;
}

const root = document.documentElement;
const reduced = matchMedia("(prefers-reduced-motion: reduce)");
const isStill = () => root.dataset.low === "1" || reduced.matches;

export function startHero(canvas: HTMLCanvasElement, section: HTMLElement, fade: HTMLElement[]) {
  let raf = 0;
  let visible = true;
  let mx = 0.5;
  let my = 0.5;
  let seededFor = "";
  let stars: Star[] = [];
  let parts: { k: number; sp: number; s: number }[] = [];
  let shoot: { x: number; y: number; t0: number } | null = null;

  addEventListener(
    "pointermove",
    (e) => {
      mx = e.clientX / innerWidth;
      my = e.clientY / innerHeight;
      if (isStill()) draw(performance.now());
    },
    { passive: true },
  );

  const seed = (W: number, H: number) => {
    const r = rng(7);
    stars = Array.from({ length: Math.round((W * H) / 5200) }, () => ({
      x: r(),
      y: r(),
      z: 0.3 + r() * 0.7,
      ph: r() * 6.28,
      sp: 0.6 + r() * 1.8,
      gold: r() < 0.18,
    }));
    parts = Array.from({ length: 34 }, (_, i) => ({ k: i / 34, sp: 0.07 + r() * 0.05, s: 0.6 + r() * 0.7 }));
    seededFor = `${W}x${H}`;
  };

  /** How far down the well the reader is: 0 at the top of the page, 1 when the hero ends. */
  const progress = () => {
    const r = section.getBoundingClientRect();
    const span = r.height - innerHeight;
    return span > 0 ? clamp(-r.top / span, 0, 1) : 0;
  };

  const draw = (now: number) => {
    const P = paletteNow();
    const { ctx, W, H } = fitCanvas(canvas);
    if (seededFor !== `${W}x${H}`) seed(W, H);
    const time = now / 1000;
    const still = isStill();
    const rtl = root.dir === "rtl";
    const S = clamp(W / 1000, 0.55, 1.15);
    const yH = H * 0.62;
    const cx = W / 2;
    const flip = (x: number) => (rtl ? W - x : x);

    const p = still ? 0 : progress();
    const e = easeInOut(p);
    const fx = cx;
    const fy = yH + 40 * S;
    const zoom = 1 + e * 3.2;
    const sx = lerp(fx, W / 2, e);
    const sy = lerp(fy, H * 0.34, e);

    // The words above the horizon fade as the camera goes down.
    const t = 1 - smooth(0, 0.28, p);
    for (const el of fade) {
      el.style.opacity = String(t);
      el.toggleAttribute("inert", t < 0.4);
    }

    ctx.save();
    ctx.clearRect(0, 0, W, H);
    ctx.translate(sx, sy);
    ctx.scale(zoom, zoom);
    ctx.translate(-fx, -fy);

    const sky = ctx.createLinearGradient(0, 0, 0, yH);
    sky.addColorStop(0, P.skyTop);
    sky.addColorStop(0.7, P.bg);
    sky.addColorStop(1, P.s2);
    ctx.fillStyle = sky;
    ctx.fillRect(-W, -H * 2, W * 3, yH + H * 2);

    const glow = ctx.createRadialGradient(cx, yH, 0, cx, yH, W * 0.6);
    glow.addColorStop(0, rgba(P.gold, P.night ? 0.16 : 0.3));
    glow.addColorStop(1, rgba(P.gold, 0));
    ctx.fillStyle = glow;
    ctx.fillRect(-W, 0, W * 3, yH);

    const px = (mx - 0.5) * 14;
    const py = (my - 0.5) * 8;
    for (const s of stars) {
      const y = s.y * (yH - 40);
      if (y > yH - 30) continue;
      const tw = still ? 0.8 : 0.55 + 0.45 * Math.sin(time * s.sp + s.ph);
      const a = (P.night ? 0.9 : 0.25) * s.z * tw * (1 - (y / yH) * 0.6);
      ctx.fillStyle = s.gold ? rgba(P.gold, a) : P.night ? `rgba(233,240,255,${a})` : rgba(P.gold, a * 0.7);
      ctx.beginPath();
      ctx.arc(s.x * W + px * s.z, y + py * s.z, s.z * 1.3, 0, 6.28);
      ctx.fill();
    }

    if (P.night && !still && p < 0.2) {
      if (!shoot && Math.random() < 0.004) shoot = { x: Math.random() * W * 0.7, y: Math.random() * yH * 0.35, t0: time };
      if (shoot) {
        const k = (time - shoot.t0) / 0.9;
        if (k > 1) shoot = null;
        else {
          const x = shoot.x + k * 260;
          const y = shoot.y + k * 90;
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

    const dune = (amp: number, base: number, freq: number, off: number, color: string, par: number) => {
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.moveTo(-W, yH);
      for (let x = -W; x <= W * 2; x += 12) {
        const y = yH - base - amp * (0.6 * Math.sin(x * freq + off) + 0.4 * Math.sin(x * freq * 2.3 + off * 1.7));
        ctx.lineTo(x + px * par, Math.min(yH, y));
      }
      ctx.lineTo(W * 2, yH);
      ctx.closePath();
      ctx.fill();
    };
    dune(22 * S, 18 * S, 0.004, 1.2, rgba(P.night ? "#14294a" : "#e8cfa0", P.night ? 0.8 : 0.9), 0.5);
    dune(14 * S, 4 * S, 0.007, 3.1, P.night ? "#0e2036" : "#dfc28c", 1);

    // The earth below the horizon: strata, deeper and denser as the camera goes down.
    const earth = ctx.createLinearGradient(0, yH, 0, H * 2.4);
    earth.addColorStop(0, P.earth);
    earth.addColorStop(1, P.skyTop);
    ctx.fillStyle = earth;
    ctx.fillRect(-W, yH, W * 3, H * 3);
    ctx.strokeStyle = rgba(P.gold, 0.09);
    ctx.lineWidth = 1;
    for (let i = 1; i < 14; i++) {
      const y = yH + i * 34 * S + i * i * 3;
      ctx.beginPath();
      ctx.moveTo(-W, y);
      ctx.lineTo(W * 2, y + 6);
      ctx.stroke();
    }

    const c0 = { x: flip(cx - 300 * S), y: yH + 62 * S };
    const c1 = { x: flip(cx + 270 * S), y: yH + 104 * S };
    const chY = (x: number) => c0.y + ((x - c0.x) / (c1.x - c0.x)) * (c1.y - c0.y);
    const wells = [-170, 0, 170].map((d) => flip(cx + d * S));
    ctx.strokeStyle = rgba(P.gold, 0.5);
    ctx.lineWidth = 5 * S;
    ctx.lineCap = "round";
    for (const x of wells) {
      ctx.beginPath();
      ctx.moveTo(x, yH);
      ctx.lineTo(x, chY(x));
      ctx.stroke();
    }
    const channel = (width: number, style: string | CanvasGradient) => {
      ctx.strokeStyle = style;
      ctx.lineWidth = width;
      ctx.beginPath();
      for (let i = 0; i <= 60; i++) {
        const x = lerp(c0.x, c1.x, i / 60);
        const y = chY(x);
        if (i) ctx.lineTo(x, y);
        else ctx.moveTo(x, y);
      }
      ctx.stroke();
    };
    channel(30 * S, P.night ? "#06121f" : "#cdb489");
    const wg = ctx.createLinearGradient(c0.x, 0, c1.x, 0);
    wg.addColorStop(0, P.waterDeep);
    wg.addColorStop(0.6, P.water);
    wg.addColorStop(1, P.waterBright);
    channel(18 * S, wg);
    ctx.save();
    ctx.setLineDash([14 * S, 18 * S]);
    ctx.lineDashOffset = still ? 0 : -time * 40 * S * (rtl ? -1 : 1);
    channel(3 * S, P.night ? "rgba(232,255,252,.75)" : "rgba(255,255,255,.7)");
    ctx.restore();

    if (!still) {
      ctx.globalCompositeOperation = P.night ? "lighter" : "source-over";
      for (const q of parts) {
        q.k = (q.k + q.sp / 60) % 1;
        const x = lerp(c0.x, c1.x, q.k);
        const y = chY(x) + Math.sin(q.k * 40 + q.s * 9) * 3 * S;
        const r = 9 * S * q.s;
        ctx.drawImage(P.sprite, x - r, y - r, r * 2, r * 2);
      }
      ctx.globalCompositeOperation = "source-over";
    }

    ctx.fillStyle = P.gold;
    for (const x of wells) {
      ctx.beginPath();
      ctx.ellipse(x, yH, 20 * S, 14 * S, 0, Math.PI, 0);
      ctx.fill();
    }
    ctx.strokeStyle = P.gold;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(-W, yH);
    ctx.lineTo(W * 2, yH);
    ctx.stroke();

    const ox = c1.x + (rtl ? -34 : 34) * S;
    const oy = c1.y + 6 * S;
    const pulse = still ? 1 : 1 + 0.12 * Math.sin(time * 3);
    const og = ctx.createRadialGradient(ox, oy, 0, ox, oy, 60 * S * pulse);
    og.addColorStop(0, rgba(P.waterBright, 0.45));
    og.addColorStop(1, rgba(P.waterBright, 0));
    ctx.fillStyle = og;
    ctx.fillRect(ox - 80 * S, oy - 80 * S, 160 * S, 160 * S);
    ctx.fillStyle = P.waterBright;
    ctx.strokeStyle = P.waterBright;
    ctx.lineWidth = 6 * S;
    ctx.lineJoin = "round";
    const d = rtl ? -1 : 1;
    ctx.beginPath();
    ctx.moveTo(ox - 16 * S * d, oy - 20 * S);
    ctx.lineTo(ox + 20 * S * d, oy);
    ctx.lineTo(ox - 16 * S * d, oy + 20 * S);
    ctx.closePath();
    ctx.fill();
    ctx.stroke();
    ctx.restore();

    // The end of the descent: the earth fills the view and becomes the next stratum.
    if (p > 0) {
      ctx.fillStyle = rgba(P.bg, smooth(0.55, 1, p));
      ctx.fillRect(0, 0, W, H);
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
