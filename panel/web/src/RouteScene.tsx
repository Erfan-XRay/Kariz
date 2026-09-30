// A tunnel drawn as one channel between two wells: water flowing when it is up, still
// water and a pulse at the break when it is broken, an empty channel when it is stopped.
import { useEffect, useRef } from "react";
import { clamp, fitCanvas, paletteNow, rgba, uiFont } from "./draw";
import type { Tunnel } from "./derive";
import { rateParts } from "./derive";
import { useApp } from "./store";

export function RouteScene({ tunnel }: { tunnel: Tunnel }) {
  const { t, num, low } = useApp();
  const canvas = useRef<HTMLCanvasElement>(null);
  const live = useRef({ tunnel, t, num, low });
  live.current = { tunnel, t, num, low };

  useEffect(() => {
    const cv = canvas.current;
    if (!cv) return;
    type Part = { k: number; dir: number; s: number; j: number };
    const parts: Part[] = [];
    let raf = 0;
    let last = performance.now();
    const draw = (now: number) => {
      const dt = Math.min(0.05, (now - last) / 1000);
      last = now;
      const { tunnel: tn, t, num, low } = live.current;
      const P = paletteNow();
      const { ctx, W, H } = fitCanvas(cv);
      const rtl = document.documentElement.dir === "rtl";
      const time = now / 1000;
      const yH = 58;
      const x0 = rtl ? W - 90 : 90;
      const x1 = rtl ? 90 : W - 90;
      const y0 = 118;
      const y1 = 124;
      ctx.clearRect(0, 0, W, H);
      const g = ctx.createLinearGradient(0, yH, 0, H);
      g.addColorStop(0, P.earth);
      g.addColorStop(1, P.bg);
      ctx.fillStyle = g;
      ctx.fillRect(0, yH, W, H - yH);
      ctx.strokeStyle = rgba(P.gold, 0.5);
      ctx.lineWidth = 4;
      ctx.lineCap = "round";
      for (const [x, y] of [
        [x0, y0],
        [x1, y1],
      ]) {
        ctx.beginPath();
        ctx.moveTo(x, yH);
        ctx.lineTo(x, y);
        ctx.stroke();
      }
      const pt = (k: number) => ({ x: x0 + (x1 - x0) * k, y: y0 + (y1 - y0) * k + Math.sin(k * Math.PI) * 10 });
      const stroke = (from: number, to: number, w: number, col: string | CanvasGradient) => {
        ctx.strokeStyle = col;
        ctx.lineWidth = w;
        ctx.beginPath();
        for (let i = 0; i <= 50; i++) {
          const p = pt(from + ((to - from) * i) / 50);
          if (i) ctx.lineTo(p.x, p.y);
          else ctx.moveTo(p.x, p.y);
        }
        ctx.stroke();
      };
      stroke(0, 1, 18, P.night ? "#06121f" : "#cdb489");
      const reach = tn.state === "down" ? 0.55 : tn.state === "off" ? 0 : 1;
      if (reach > 0) {
        const wg = ctx.createLinearGradient(x0, 0, x1, 0);
        wg.addColorStop(0, P.waterDeep);
        wg.addColorStop(1, P.waterBright);
        stroke(0, reach, 10, wg);
      }
      if (tn.state !== "up") {
        ctx.save();
        ctx.setLineDash([4, 6]);
        stroke(reach, 1, 1.2, tn.state === "down" ? rgba(P.danger, 0.6) : rgba(P.text3, 0.6));
        ctx.restore();
      }
      if (tn.state === "down") {
        const p = pt(reach);
        const k = low ? 0.5 : (time * 0.8) % 1;
        ctx.strokeStyle = rgba(P.danger, 1 - k);
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.arc(p.x, p.y, 6 + k * 18, 0, 6.28);
        ctx.stroke();
        ctx.fillStyle = P.danger;
        ctx.beginPath();
        ctx.arc(p.x, p.y, 5, 0, 6.28);
        ctx.fill();
      }
      // particles: most of the water comes back to the entry (downloads)
      if (tn.state === "up" && !low) {
        const want = Math.round(clamp(10 + tn.rate / 6, 10, 90));
        while (parts.length < want) parts.push({ k: Math.random(), dir: Math.random() < 0.8 ? -1 : 1, s: 0.5 + Math.random() * 0.7, j: Math.random() * 6 });
        parts.length = want;
        const v = clamp(0.12 + tn.rate / 2500, 0.12, 0.5);
        ctx.globalCompositeOperation = P.night ? "lighter" : "source-over";
        for (const q of parts) {
          q.k = (q.k + q.dir * v * dt + 1) % 1;
          const p = pt(q.k);
          const r = (q.dir < 0 ? 8 : 6) * q.s;
          ctx.globalAlpha = q.dir < 0 ? 1 : 0.55;
          ctx.drawImage(P.sprite, p.x - r, p.y + Math.sin(time * 3 + q.j) * 2 - r, r * 2, r * 2);
        }
        ctx.globalAlpha = 1;
        ctx.globalCompositeOperation = "source-over";
      }
      ctx.strokeStyle = P.gold;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.moveTo(0, yH);
      ctx.lineTo(W, yH);
      ctx.stroke();
      ctx.fillStyle = P.gold;
      for (const x of [x0, x1]) {
        ctx.beginPath();
        ctx.ellipse(x, yH, 18, 12, 0, Math.PI, 0);
        ctx.fill();
      }
      ctx.textAlign = "center";
      ctx.direction = rtl ? "rtl" : "ltr";
      const label = (x: number, name: string, role: string, note: string) => {
        ctx.font = uiFont(600, 14);
        ctx.fillStyle = P.text;
        ctx.fillText(name, x, yH - 30);
        ctx.font = uiFont(400, 11);
        ctx.fillStyle = P.text3;
        ctx.fillText(role, x, yH - 14);
        ctx.fillText(note, x, y1 + 30);
      };
      label(x0, tn.entry?.server.name ?? "?", t("tun.side.entry"), t("td.users"));
      label(x1, tn.exit?.server.name ?? "?", t("tun.side.exit"), t("td.targets"));
      if (tn.state === "up") {
        const r = rateParts(tn.rate);
        const p = pt(0.5);
        ctx.font = uiFont(600, 13);
        ctx.fillStyle = P.text;
        // a number and its unit read left to right in either language
        ctx.direction = "ltr";
        ctx.fillText(`${num(r.value, r.decimals)} ${r.unit}`, p.x, p.y - 20);
      }
      raf = low ? 0 : requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    // In low power mode it is drawn once per change instead of every frame.
    const id = low ? setInterval(() => requestAnimationFrame(draw), 2500) : 0;
    return () => {
      cancelAnimationFrame(raf);
      if (id) clearInterval(id);
    };
  }, [low]);

  return <canvas ref={canvas} aria-hidden="true" />;
}
