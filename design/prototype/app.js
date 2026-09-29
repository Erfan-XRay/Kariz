/* Kariz panel prototype: boot, login, the descent, the live map. No dependencies. */
(() => {
  "use strict";

  // ------------------------------------------------------------------ strings

  const T = {
    fa: {
      "boot.sub": "پنل تانل",
      "theme.toDawn": "سحر",
      "theme.toNight": "شب",
      "login.title": "کاریز",
      "login.sub": "آب، راهش را از زیر زمین پیدا می‌کند.",
      "login.password": "رمز مدیر",
      "login.enter": "ورود",
      "login.or": "یا",
      "login.link": "لینک یک‌بارمصرف دارم",
      "login.checking": "در حال بررسی لینک…",
      "login.foot": "ساخت لینک ورود روی سرور:",
      "login.empty": "رمز را وارد کنید.",
      "login.wrong": "رمز درست نیست. ۴ تلاش دیگر مانده است.",
      "login.show": "نمایش رمز",
      "nav.map": "نقشه",
      "nav.servers": "سرورها",
      "nav.tunnels": "تانل‌ها",
      "nav.logs": "لاگ",
      "nav.settings": "تنظیمات",
      "nav.logout": "خروج",
      "page.map": "نقشه",
      "page.mapSub": "۵ سرور، ۵ تانل",
      "page.servers": "سرورها",
      "page.tunnels": "تانل‌ها",
      "page.logs": "لاگ",
      "page.settings": "تنظیمات",
      "page.soon": "در مرحلهٔ ۹.۳",
      search: "جستجو یا اجرای دستور",
      "st.up": "جاری",
      "st.down": "قطع",
      "st.off": "خاموش",
      "st.warn": "هشدار",
      "m.through": "ترافیک همین حالا",
      "m.conns": "اتصال‌های باز",
      "m.connsNote": "استریم TCP و جریان UDP",
      "m.tunnels": "تانل‌های جاری",
      "m.servers": "سرورهای آنلاین",
      "m.serversNote": "همه روی کاریز 0.6.1",
      "m.of": "از",
      "m.rateNote": "۱۲٪ بیشتر از یک ساعت پیش",
      "m.broken": "قطع:",
      "t.title": "تانل‌ها",
      "t.hint": "یکی را خاموش کنید تا خالی شدن کانالش را ببینید",
      "t.name": "نام",
      "t.route": "مسیر",
      "t.transport": "ترنسپورت",
      "t.state": "وضعیت",
      "t.traffic": "یک دقیقهٔ اخیر",
      "t.rate": "الان",
      "t.onoff": "روشن / خاموش",
      "tip.down": "دانلود",
      "tip.up": "آپلود",
      "tip.lat": "تأخیر",
      "tip.conns": "اتصال",
      "tip.error": "خطا",
      "tip.cpu": "پردازنده",
      "tip.ram": "حافظه",
      "tip.tunnels": "تانل",
      "tip.role": "نقش",
      "role.entry": "ورودی",
      "role.exit": "خروجی",
      placeholder: "این صفحه در مرحلهٔ ۹.۳ طراحی می‌شود.",
      "proto.label": "نمونهٔ اولیه · رمز: kariz",
      "proto.replay": "پخش دوباره",
      "pal.placeholder": "دستور یا صفحه…",
      "pal.new": "تانل جدید",
      "pal.goto": "برو به",
      "pal.theme": "عوض کردن تم",
      "pal.lang": "English",
      "pal.low": "حالت کم‌مصرف",
      "pal.speed": "اسپیدتست روی تانل main",
      "pal.logout": "خروج",
      "toast.wizard": "wizard ساخت تانل در مرحلهٔ ۹.۳ طراحی می‌شود.",
      "toast.open": "جزئیات تانل در مرحلهٔ ۹.۳ طراحی می‌شود:",
      "toast.speedRun": "اسپیدتست روی main شروع شد…",
      "toast.speedDone": "main: ↓ ۸۴۲ Mbps  ↑ ۱۹۶ Mbps  ·  ۳۹ ms",
      "toast.low.on": "حالت کم‌مصرف روشن شد: ذره‌ها و انیمیشن‌ها متوقف شدند.",
      "toast.low.off": "حالت کم‌مصرف خاموش شد.",
      "toast.welcome": "خوش آمدید. ورود با لینک یک‌بارمصرف؛ لینک باطل شد.",
      stopped: "خاموش",
      "alt.prefix": "نقشهٔ تانل‌ها:",
    },
    en: {
      "boot.sub": "tunnel panel",
      "theme.toDawn": "Dawn",
      "theme.toNight": "Night",
      "login.title": "Kariz",
      "login.sub": "Water finds its way underground.",
      "login.password": "Admin password",
      "login.enter": "Enter",
      "login.or": "or",
      "login.link": "I have a one-time link",
      "login.checking": "Checking the link…",
      "login.foot": "Make a one-time link on the server:",
      "login.empty": "Enter the password.",
      "login.wrong": "Wrong password. 4 tries left.",
      "login.show": "Show password",
      "nav.map": "Map",
      "nav.servers": "Servers",
      "nav.tunnels": "Tunnels",
      "nav.logs": "Logs",
      "nav.settings": "Settings",
      "nav.logout": "Sign out",
      "page.map": "Map",
      "page.mapSub": "5 servers, 5 tunnels",
      "page.servers": "Servers",
      "page.tunnels": "Tunnels",
      "page.logs": "Logs",
      "page.settings": "Settings",
      "page.soon": "in step 9.3",
      search: "Search or run a command",
      "st.up": "flowing",
      "st.down": "broken",
      "st.off": "stopped",
      "st.warn": "warning",
      "m.through": "Throughput now",
      "m.conns": "Open connections",
      "m.connsNote": "TCP streams and UDP flows",
      "m.tunnels": "Tunnels flowing",
      "m.servers": "Servers online",
      "m.serversNote": "all on Kariz 0.6.1",
      "m.of": "of",
      "m.rateNote": "12% more than an hour ago",
      "m.broken": "Broken:",
      "t.title": "Tunnels",
      "t.hint": "switch one off to watch its channel drain",
      "t.name": "Name",
      "t.route": "Route",
      "t.transport": "Transport",
      "t.state": "State",
      "t.traffic": "Last minute",
      "t.rate": "Now",
      "t.onoff": "On / off",
      "tip.down": "Down",
      "tip.up": "Up",
      "tip.lat": "Latency",
      "tip.conns": "Connections",
      "tip.error": "Error",
      "tip.cpu": "CPU",
      "tip.ram": "Memory",
      "tip.tunnels": "Tunnels",
      "tip.role": "Role",
      "role.entry": "entry",
      "role.exit": "exit",
      placeholder: "This screen is designed in step 9.3.",
      "proto.label": "Prototype · password: kariz",
      "proto.replay": "Replay intro",
      "pal.placeholder": "A command or a page…",
      "pal.new": "New tunnel",
      "pal.goto": "Go to",
      "pal.theme": "Switch theme",
      "pal.lang": "فارسی",
      "pal.low": "Low-power mode",
      "pal.speed": "Speed test on tunnel main",
      "pal.logout": "Sign out",
      "toast.wizard": "The tunnel wizard is designed in step 9.3.",
      "toast.open": "Tunnel detail is designed in step 9.3:",
      "toast.speedRun": "Speed test on main started…",
      "toast.speedDone": "main: ↓ 842 Mbps  ↑ 196 Mbps  ·  39 ms",
      "toast.low.on": "Low-power mode on: particles and motion stopped.",
      "toast.low.off": "Low-power mode off.",
      "toast.welcome": "Welcome. Signed in with a one-time link; the link is now spent.",
      stopped: "stopped",
      "alt.prefix": "Tunnel map:",
    },
  };

  // ------------------------------------------------------------------ data

  const SERVERS = [
    { id: "teh", name: "tehran-1", loc: { fa: "تهران", en: "Tehran" }, ip: "203.0.113.5", role: "entry", st: "up", cpu: 23, ram: 41 },
    { id: "tbz", name: "tabriz-edge", loc: { fa: "تبریز", en: "Tabriz" }, ip: "203.0.113.18", role: "entry", st: "up", cpu: 12, ram: 28 },
    { id: "fra", name: "frankfurt", loc: { fa: "فرانکفورت", en: "Frankfurt" }, ip: "198.51.100.7", role: "exit", st: "up", cpu: 31, ram: 37 },
    { id: "hel", name: "helsinki", loc: { fa: "هلسینکی", en: "Helsinki" }, ip: "198.51.100.41", role: "exit", st: "up", cpu: 9, ram: 22 },
    { id: "ams", name: "amsterdam", loc: { fa: "آمستردام", en: "Amsterdam" }, ip: "2001:db8::7", role: "exit", st: "warn", cpu: 4, ram: 19 },
  ];

  const TUNNELS = [
    { id: "main", a: "teh", b: "fra", tr: "tcpmux", prof: "ultraspeed", st: "up", base: 420, conns: 812, lat: 38, level: 0 },
    { id: "games", a: "teh", b: "hel", tr: "kcp", prof: "gaming", st: "up", base: 36, conns: 146, lat: 52, level: 1 },
    { id: "cdn", a: "tbz", b: "fra", tr: "wss", prof: "balanced", st: "up", base: 128, conns: 301, lat: 61, level: 2 },
    { id: "backup", a: "tbz", b: "ams", tr: "quic", prof: "balanced", st: "down", base: 0, conns: 0, lat: 0, level: 3,
      err: { fa: "handshake رد شد: توکن دو طرف یکی نیست", en: "handshake rejected: the two sides' tokens differ" } },
    { id: "office", a: "teh", b: "ams", tr: "tcp", prof: "balanced", st: "off", base: 22, conns: 40, lat: 44, level: 4 },
  ];

  for (const t of TUNNELS) {
    t.fill = 0; // how much of the channel holds water, 0..1
    t.noise = Math.random();
    t.down = 0;
    t.up = 0;
    t.open = 0;
    t.hist = Array.from({ length: 60 }, () => 0);
    t.parts = [];
  }

  // ------------------------------------------------------------------ state and helpers

  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];
  const html = document.documentElement;
  const reduced = matchMedia("(prefers-reduced-motion: reduce)");

  const state = {
    lang: "fa",
    theme: "night",
    low: reduced.matches,
    page: "map",
    hot: null, // hovered tunnel id
  };

  const t = (k) => T[state.lang][k] ?? T.en[k] ?? k;
  const FA_DIGITS = "۰۱۲۳۴۵۶۷۸۹";

  function localDigits(s) {
    if (state.lang !== "fa") return s;
    return s.replace(/\d/g, (d) => FA_DIGITS[d]).replace(/\./g, "٫").replace(/,/g, "٬");
  }

  function fmt(n, dec = 0) {
    const [i, f] = Math.abs(n).toFixed(dec).split(".");
    const grouped = i.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
    return localDigits((n < 0 ? "-" : "") + grouped + (f ? "." + f : ""));
  }

  function fmtRate(mbps) {
    if (mbps >= 1000) return { v: fmt(mbps / 1000, 2), u: "Gbps" };
    return { v: fmt(mbps, mbps < 100 ? 1 : 0), u: "Mbps" };
  }

  const clamp = (x, a, b) => Math.max(a, Math.min(b, x));
  const lerp = (a, b, k) => a + (b - a) * k;
  const easeInOut = (x) => (x < 0.5 ? 4 * x * x * x : 1 - Math.pow(-2 * x + 2, 3) / 2);
  const smooth = (a, b, x) => {
    const k = clamp((x - a) / (b - a), 0, 1);
    return k * k * (3 - 2 * k);
  };

  let P = {}; // the palette, read from CSS
  function readPalette() {
    const cs = getComputedStyle(html);
    const v = (n) => cs.getPropertyValue(n).trim();
    P = {
      bg: v("--bg"), skyTop: v("--sky-top"), s1: v("--stratum-1"), s2: v("--stratum-2"), s3: v("--stratum-3"),
      earth: v("--earth"), text: v("--text"), text2: v("--text-2"), text3: v("--text-3"),
      accent: v("--accent"), water: v("--water"), waterBright: v("--water-bright"), waterDeep: v("--water-deep"),
      danger: v("--danger"), warn: v("--warn"), star: v("--star"),
      night: state.theme === "night",
      gold: state.theme === "night" ? "#e9c46a" : "#9a6a10",
    };
    sprite = makeSprite(P.waterBright, P.water);
    redSprite = makeSprite("#ffb3b3", P.danger);
  }

  function rgba(hex, a) {
    const h = hex.replace("#", "");
    const n = parseInt(h.length === 3 ? h.replace(/./g, "$&$&") : h, 16);
    return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`;
  }

  let sprite, redSprite;
  function makeSprite(core, edge) {
    const c = document.createElement("canvas");
    c.width = c.height = 32;
    const g = c.getContext("2d");
    const grd = g.createRadialGradient(16, 16, 0, 16, 16, 16);
    grd.addColorStop(0, core);
    grd.addColorStop(0.35, rgba(edge, 0.55));
    grd.addColorStop(1, rgba(edge, 0));
    g.fillStyle = grd;
    g.fillRect(0, 0, 32, 32);
    return c;
  }

  function fitCanvas(canvas) {
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const r = canvas.getBoundingClientRect();
    const w = Math.max(1, Math.round(r.width * dpr));
    const h = Math.max(1, Math.round(r.height * dpr));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    const ctx = canvas.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    return { ctx, W: r.width, H: r.height };
  }

  const qb = (p0, c, p1, k) => {
    const m = 1 - k;
    return { x: m * m * p0.x + 2 * m * k * c.x + k * k * p1.x, y: m * m * p0.y + 2 * m * k * c.y + k * k * p1.y };
  };

  const uiFont = (w, px) => `${w} ${px}px "Space Grotesk", "Estedad", system-ui, sans-serif`;
  const monoFont = (px) => `400 ${px}px "IBM Plex Mono", ui-monospace, monospace`;

  // random but repeatable
  function rng(seed) {
    return () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646;
  }

  // ------------------------------------------------------------------ i18n, theme, low power

  function applyLang() {
    html.lang = state.lang;
    html.dir = state.lang === "fa" ? "rtl" : "ltr";
    for (const el of $$("[data-i18n]")) el.textContent = t(el.dataset.i18n);
    for (const b of $$('[data-act="lang"]')) b.textContent = b.classList.contains("chip") ? (state.lang === "fa" ? "English" : "فارسی") : state.lang === "fa" ? "EN" : "فا";
    $("#pw-eye").setAttribute("aria-label", t("login.show"));
    $("#palette-input").placeholder = t("pal.placeholder");
    applyThemeLabels();
    setPage(state.page, true);
    renderRows();
    odoAll(true);
  }

  function applyThemeLabels() {
    for (const b of $$('.chip[data-act="theme"]')) b.textContent = state.theme === "night" ? t("theme.toDawn") : t("theme.toNight");
    for (const b of $$('.square-btn[data-act="theme"]')) b.innerHTML = `<svg><use href="#i-${state.theme === "night" ? "sun" : "moon"}"/></svg>`;
  }

  function toggleTheme() {
    state.theme = state.theme === "night" ? "dawn" : "night";
    html.dataset.theme = state.theme;
    readPalette();
    applyThemeLabels();
    drawSparks();
  }

  function toggleLang() {
    state.lang = state.lang === "fa" ? "en" : "fa";
    applyLang();
  }

  function setLow(on, announce) {
    state.low = on;
    for (const b of $$('[data-act="lowpower"]')) b.setAttribute("aria-pressed", String(on));
    if (announce) toast(t(on ? "toast.low.on" : "toast.low.off"));
  }

  // ------------------------------------------------------------------ the qanat loader

  const LOADER = `
    <path class="bed" d="M30 58 L158 76"/>
    <path class="flow" pathLength="1" d="M30 58 L158 76"/>
    <path class="shaft" d="M38 26 V58 M84 26 V64 M130 26 V70"/>
    <path class="ground" d="M8 26 H172"/>
    <path class="mound" d="M30 26 a8 6 0 0 1 16 0Z M76 26 a8 6 0 0 1 16 0Z M122 26 a8 6 0 0 1 16 0Z"/>
    <circle class="drop" cx="38" cy="26" r="4"/>
    <path class="out" d="M164 68 L178 77 L164 86 Z"/>`;
  for (const s of $$("svg.qloader")) s.innerHTML = LOADER;

  // ------------------------------------------------------------------ screens

  function show(id) {
    for (const s of $$(".screen")) s.classList.toggle("is-on", s.id === id);
  }

  function boot() {
    const old = $("#boot");
    const fresh = old.cloneNode(true); // restarts the CSS animations
    old.replaceWith(fresh);
    show("boot");
    setTimeout(() => {
      openLogin();
      if (location.hash.startsWith("#t=")) {
        history.replaceState(null, "", location.pathname); // the token never stays in the address bar
        linkLogin(true);
      }
    }, state.low ? 700 : 2300);
  }

  // ------------------------------------------------------------------ login

  const login = {
    canvas: $("#login-scene"),
    running: false,
    descent: -1, // start time of the descent, or -1
    ripple: -1,
    mx: 0.5,
    my: 0.5,
    stars: [],
    parts: [],
    shoot: null,
  };

  function openLogin() {
    const form = $("#login-form");
    form.classList.remove("is-leaving", "is-busy");
    $("#pw").value = "";
    $("#pw").removeAttribute("aria-invalid");
    $("#pw-err").textContent = "";
    login.descent = -1;
    show("login");
    if (!login.running) {
      login.running = true;
      requestAnimationFrame(loginFrame);
    }
    setTimeout(() => $("#pw").focus({ preventScroll: true }), 400);
  }

  function seedLogin(W, H) {
    const r = rng(7);
    login.stars = Array.from({ length: Math.round((W * H) / 5200) }, () => ({
      x: r(), y: r(), z: 0.3 + r() * 0.7, ph: r() * 6.28, sp: 0.6 + r() * 1.8, gold: r() < 0.18,
    }));
    login.parts = Array.from({ length: 34 }, (_, i) => ({ k: i / 34, sp: 0.07 + r() * 0.05, s: 0.6 + r() * 0.7 }));
    login.seeded = `${W}x${H}`;
  }

  function loginFrame(now) {
    if (!$("#login").classList.contains("is-on") && login.descent < 0) {
      login.running = false;
      return;
    }
    const { ctx, W, H } = fitCanvas(login.canvas);
    if (login.seeded !== `${W}x${H}`) seedLogin(W, H);
    const time = now / 1000;
    const still = state.low;
    const rtl = html.dir === "rtl";
    const S = clamp(W / 1000, 0.55, 1.15); // scene scale
    const yH = H * (H < 860 ? 0.74 : 0.68);
    const cx = W / 2;
    const flip = (x) => (rtl ? W - x : x);

    // camera for the descent
    let p = 0;
    if (login.descent >= 0) p = clamp((now - login.descent) / (state.low ? 250 : 1200), 0, 1);
    const e = easeInOut(p);
    const fx = cx, fy = yH + 40 * S;
    const zoom = 1 + e * 2.6;
    const sx = lerp(fx, W / 2, e), sy = lerp(fy, H * 0.3, e);

    ctx.save();
    ctx.clearRect(0, 0, W, H);
    ctx.translate(sx, sy);
    ctx.scale(zoom, zoom);
    ctx.translate(-fx, -fy);

    // sky
    const sky = ctx.createLinearGradient(0, 0, 0, yH);
    sky.addColorStop(0, P.skyTop);
    sky.addColorStop(0.7, P.bg);
    sky.addColorStop(1, P.s2);
    ctx.fillStyle = sky;
    ctx.fillRect(-W, -H, W * 3, yH + H);

    // horizon glow
    const glow = ctx.createRadialGradient(cx, yH, 0, cx, yH, W * 0.6);
    glow.addColorStop(0, rgba(P.gold, P.night ? 0.16 : 0.3));
    glow.addColorStop(1, rgba(P.gold, 0));
    ctx.fillStyle = glow;
    ctx.fillRect(-W, 0, W * 3, yH);

    // stars, with a little parallax
    const px = (login.mx - 0.5) * 14, py = (login.my - 0.5) * 8;
    for (const s of login.stars) {
      const y = s.y * (yH - 40);
      if (y > yH - 30) continue;
      const tw = still ? 0.8 : 0.55 + 0.45 * Math.sin(time * s.sp + s.ph);
      const a = (P.night ? 0.9 : 0.25) * s.z * tw * (1 - y / yH * 0.6);
      ctx.fillStyle = s.gold ? rgba(P.gold, a) : P.night ? `rgba(233,240,255,${a})` : rgba(P.gold, a * 0.7);
      ctx.beginPath();
      ctx.arc(s.x * W + px * s.z, y + py * s.z, s.z * 1.3, 0, 6.28);
      ctx.fill();
    }

    // a shooting star now and then (night only)
    if (P.night && !still) {
      if (!login.shoot && Math.random() < 0.004) login.shoot = { x: Math.random() * W * 0.7, y: Math.random() * yH * 0.35, t0: time };
      if (login.shoot) {
        const k = (time - login.shoot.t0) / 0.9;
        if (k > 1) login.shoot = null;
        else {
          const x = login.shoot.x + k * 260, y = login.shoot.y + k * 90;
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

    // dunes
    const dune = (amp, base, freq, off, color, par) => {
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

    // earth
    const earth = ctx.createLinearGradient(0, yH, 0, H);
    earth.addColorStop(0, P.earth);
    earth.addColorStop(1, P.skyTop);
    ctx.fillStyle = earth;
    ctx.fillRect(-W, yH, W * 3, H * 2);
    ctx.strokeStyle = rgba(P.gold, 0.07);
    ctx.lineWidth = 1;
    for (let i = 1; i < 5; i++) {
      const y = yH + i * 34 * S + i * i * 3;
      ctx.beginPath();
      ctx.moveTo(-W, y);
      ctx.lineTo(W * 2, y + 6);
      ctx.stroke();
    }

    // the channel
    const c0 = { x: flip(cx - 300 * S), y: yH + 62 * S };
    const c1 = { x: flip(cx + 270 * S), y: yH + 104 * S };
    const chY = (x) => c0.y + ((x - c0.x) / (c1.x - c0.x)) * (c1.y - c0.y);
    const wells = [-170, 0, 170].map((d) => flip(cx + d * S));

    // shafts
    ctx.strokeStyle = rgba(P.gold, 0.5);
    ctx.lineWidth = 5 * S;
    ctx.lineCap = "round";
    for (const x of wells) {
      ctx.beginPath();
      ctx.moveTo(x, yH);
      ctx.lineTo(x, chY(x));
      ctx.stroke();
    }

    // ripple after a wrong password
    let rip = 0;
    if (login.ripple >= 0) {
      const dt = (now - login.ripple) / 1000;
      rip = dt > 1.6 ? 0 : Math.exp(-dt * 2.6);
      if (!rip) login.ripple = -1;
    }
    const wave = (x) => (rip ? Math.sin(x * 0.06 - time * 18) * 5 * S * rip : 0);

    const channel = (width, style) => {
      ctx.strokeStyle = style;
      ctx.lineWidth = width;
      ctx.beginPath();
      for (let i = 0; i <= 60; i++) {
        const x = lerp(c0.x, c1.x, i / 60);
        const y = chY(x) + wave(x);
        i ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
      }
      ctx.stroke();
    };
    channel(30 * S, P.night ? "#06121f" : "#cdb489");
    const wg = ctx.createLinearGradient(c0.x, 0, c1.x, 0);
    wg.addColorStop(0, P.waterDeep);
    wg.addColorStop(0.6, P.water);
    wg.addColorStop(1, P.waterBright);
    channel(18 * S, wg);
    if (rip) channel(18 * S, rgba(P.danger, rip * 0.55));

    // moving highlight
    ctx.save();
    ctx.setLineDash([14 * S, 18 * S]);
    ctx.lineDashOffset = still ? 0 : (-time * 40 * S) * (rtl ? -1 : 1);
    channel(3 * S, P.night ? "rgba(232,255,252,.75)" : "rgba(255,255,255,.7)");
    ctx.restore();

    // particles
    if (!still) {
      ctx.globalCompositeOperation = P.night ? "lighter" : "source-over";
      for (const q of login.parts) {
        q.k = (q.k + q.sp / 60) % 1;
        const x = lerp(c0.x, c1.x, q.k);
        const y = chY(x) + wave(x) + Math.sin(q.k * 40 + q.s * 9) * 3 * S;
        const r = 9 * S * q.s;
        ctx.drawImage(sprite, x - r, y - r, r * 2, r * 2);
      }
      ctx.globalCompositeOperation = "source-over";
    }

    // mounds
    ctx.fillStyle = P.gold;
    for (const x of wells) {
      ctx.beginPath();
      ctx.ellipse(x, yH, 20 * S, 14 * S, 0, Math.PI, 0);
      ctx.fill();
    }

    // horizon
    ctx.strokeStyle = P.gold;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(-W, yH);
    ctx.lineTo(W * 2, yH);
    ctx.stroke();

    // outlet: the water leaves as an arrow
    const ox = c1.x + (rtl ? -34 : 34) * S, oy = c1.y + 6 * S;
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

    // the end of the descent: the earth fills the view
    if (p > 0) {
      ctx.fillStyle = rgba(P.bg, smooth(0.5, 1, p));
      ctx.fillRect(0, 0, W, H);
    }

    if (p >= 1) {
      login.descent = -1;
      enterApp();
    }
    requestAnimationFrame(loginFrame);
  }

  function descend() {
    $("#login-form").classList.add("is-leaving");
    login.descent = performance.now();
  }

  function linkLogin(fromUrl) {
    const form = $("#login-form");
    form.classList.add("is-busy");
    setTimeout(() => {
      descend();
      state.welcome = true;
    }, fromUrl ? 1600 : 1400);
  }

  $("#login-form").addEventListener("submit", (ev) => {
    ev.preventDefault();
    const pw = $("#pw");
    const err = $("#pw-err");
    if (pw.value === "kariz") {
      err.textContent = "";
      descend();
      return;
    }
    err.textContent = t(pw.value ? "login.wrong" : "login.empty");
    pw.setAttribute("aria-invalid", "true");
    const wrap = $("#pw-wrap");
    wrap.classList.remove("shake");
    void wrap.offsetWidth;
    wrap.classList.add("shake");
    login.ripple = performance.now();
    pw.select();
  });

  $("#pw").addEventListener("input", () => {
    $("#pw").removeAttribute("aria-invalid");
    $("#pw-err").textContent = "";
  });

  $("#pw-eye").addEventListener("click", () => {
    const pw = $("#pw");
    pw.type = pw.type === "password" ? "text" : "password";
    pw.focus();
  });

  $("#link-login").addEventListener("click", () => linkLogin(false));

  addEventListener("pointermove", (e) => {
    login.mx = e.clientX / innerWidth;
    login.my = e.clientY / innerHeight;
  });

  // ------------------------------------------------------------------ app

  function enterApp() {
    const app = $("#app");
    app.classList.add("is-rising");
    show("app");
    setTimeout(() => app.classList.remove("is-rising"), 1200);
    for (const tn of TUNNELS) {
      tn.fill = 0; // channels fill as the dashboard rises
      tn.parts = [];
    }
    odoAll(true);
    if (!map.running) {
      map.running = true;
      requestAnimationFrame(mapFrame);
    }
    if (state.welcome) {
      state.welcome = false;
      setTimeout(() => toast(t("toast.welcome")), 900);
    }
  }

  function logout() {
    show("login");
    openLogin();
  }

  // pages
  const PAGES = ["map", "servers", "tunnels", "logs", "settings"];
  function setPage(id, silent) {
    state.page = id;
    for (const b of $$(".rail-item[data-page]")) {
      if (b.dataset.page === id) b.setAttribute("aria-current", "page");
      else b.removeAttribute("aria-current");
    }
    $("#page-title").textContent = t(`page.${id}`);
    $("#page-sub").textContent = id === "map" ? t("page.mapSub") : t("page.soon");
    const swap = () => {
      for (const p of $$(".page")) p.classList.toggle("is-on", p.dataset.page === (id === "map" ? "map" : "other"));
    };
    if (silent) return swap();
    const line = $("#loader-line");
    line.classList.add("is-on");
    setTimeout(() => {
      swap();
      line.classList.remove("is-on");
      $("#main").scrollTop = 0;
    }, 450);
  }

  $$(".rail-item[data-page]").forEach((b) => b.addEventListener("click", () => setPage(b.dataset.page)));

  // ------------------------------------------------------------------ simulation

  function tick() {
    for (const tn of TUNNELS) {
      tn.noise = clamp(tn.noise + (Math.random() - 0.5) * 0.25, 0, 1);
      if (tn.st === "up") {
        tn.down = tn.base * (0.7 + 0.6 * tn.noise);
        tn.up = tn.down * (0.16 + 0.06 * Math.random());
        tn.open = Math.round(tn.conns * (0.85 + 0.3 * tn.noise));
      } else {
        tn.down = tn.up = tn.open = 0;
      }
      tn.hist.push(tn.down + tn.up);
      tn.hist.shift();
    }
    updateMetrics();
    updateRowRates();
    drawSparks();
    updateAlt();
  }

  // ------------------------------------------------------------------ odometer numbers

  // Digits roll to their value. A fresh build starts at zero and rolls on the next frame.
  function odo(el, text, fresh) {
    const shape = text.replace(/[0-9۰-۹]/g, "d") + state.lang;
    const digits = state.lang === "fa" ? FA_DIGITS : "0123456789";
    el._text = text;
    el.setAttribute("aria-label", text);
    if (el.dataset.shape !== shape || fresh) {
      el.dataset.shape = shape;
      el.innerHTML = [...text]
        .map((ch) =>
          /[0-9۰-۹]/.test(ch)
            ? `<span class="odo-d"><span class="odo-c">${[...digits].map((d) => `<span>${d}</span>`).join("")}</span></span>`
            : `<span>${ch}</span>`
        )
        .join("");
      requestAnimationFrame(() => requestAnimationFrame(() => roll(el)));
      return;
    }
    roll(el);
  }

  function roll(el) {
    const text = el._text;
    if (text.replace(/[0-9۰-۹]/g, "d") + state.lang !== el.dataset.shape) return; // rebuilt since
    const cols = $$(".odo-c", el);
    let i = 0;
    for (const ch of text) {
      if (!/[0-9۰-۹]/.test(ch)) continue;
      const fa = FA_DIGITS.indexOf(ch);
      cols[i++].style.transform = `translateY(${-(fa >= 0 ? fa : Number(ch)) * 10}%)`;
    }
  }

  function updateMetrics(fresh) {
    const up = TUNNELS.filter((x) => x.st === "up");
    const total = up.reduce((s, x) => s + x.down + x.up, 0);
    const r = fmtRate(total);
    odo($("#m-rate"), r.v, fresh);
    $("#m-rate-unit").textContent = r.u;
    $("#m-rate-note").textContent = "▲ " + t("m.rateNote");
    odo($("#m-conns"), fmt(up.reduce((s, x) => s + x.open, 0)), fresh);
    odo($("#m-tun"), fmt(up.length), fresh);
    $("#m-tun-of").textContent = `${t("m.of")} ${fmt(TUNNELS.length)}`;
    const broken = TUNNELS.filter((x) => x.st === "down").map((x) => x.id);
    $("#m-tun-note").textContent = broken.length ? `${t("m.broken")} ${broken.join(", ")}` : "";
    $("#m-tun-note").style.color = broken.length ? "var(--danger)" : "";
    odo($("#m-srv"), fmt(SERVERS.length), fresh);
    $("#m-srv-of").textContent = `${t("m.of")} ${fmt(SERVERS.length)}`;
  }

  function odoAll(fresh) {
    updateMetrics(fresh);
  }

  // ------------------------------------------------------------------ tunnel rows

  const byId = (id) => SERVERS.find((s) => s.id === id);

  function stateLabel(st) {
    return { up: t("st.up"), down: t("st.down"), off: t("st.off") }[st];
  }

  function renderRows() {
    const body = $("#tunnel-rows");
    body.innerHTML = TUNNELS.map(
      (tn) => `
      <tr data-t="${tn.id}">
        <td><span class="t-name">${tn.id}</span></td>
        <td><span class="t-route"><span>${byId(tn.a).name}</span><svg><use href="#i-arrow"/></svg><span>${byId(tn.b).name}</span></span></td>
        <td class="c-transport"><span class="tag">${tn.tr}</span> <span class="tag">${tn.prof}</span></td>
        <td><span class="state ${tn.st}"><i class="dot ${tn.st}"></i>${stateLabel(tn.st)}</span></td>
        <td class="c-spark"><canvas class="spark" aria-hidden="true"></canvas></td>
        <td class="rate num"></td>
        <td><button class="switch" role="switch" aria-checked="${tn.st !== "off"}" aria-label="${tn.id}"></button></td>
      </tr>`
    ).join("");
    for (const row of $$("tr[data-t]", body)) {
      const tn = TUNNELS.find((x) => x.id === row.dataset.t);
      $(".switch", row).addEventListener("click", () => toggleTunnel(tn));
      row.addEventListener("mouseenter", () => (state.hot = tn.id));
      row.addEventListener("mouseleave", () => (state.hot = null));
    }
    updateRowRates();
    drawSparks();
  }

  function updateRowRates() {
    for (const row of $$("#tunnel-rows tr[data-t]")) {
      const tn = TUNNELS.find((x) => x.id === row.dataset.t);
      const cell = $(".rate", row);
      if (tn.st === "up") {
        const r = fmtRate(tn.down + tn.up);
        cell.textContent = `${r.v} ${r.u}`;
      } else cell.textContent = "—";
      row.classList.toggle("is-hot", state.hot === tn.id);
    }
  }

  function drawSparks() {
    for (const row of $$("#tunnel-rows tr[data-t]")) {
      const tn = TUNNELS.find((x) => x.id === row.dataset.t);
      const cv = $(".spark", row);
      if (!cv || !cv.getBoundingClientRect().width) continue;
      const { ctx, W, H } = fitCanvas(cv);
      ctx.clearRect(0, 0, W, H);
      const max = Math.max(1, ...tn.hist) * 1.15;
      const pts = tn.hist.map((v, i) => ({ x: (i / (tn.hist.length - 1)) * W, y: H - 2 - (v / max) * (H - 6) }));
      if (html.dir === "rtl") for (const q of pts) q.x = W - q.x;
      const col = tn.st === "down" ? P.danger : tn.st === "off" ? P.text3 : P.water;
      const g = ctx.createLinearGradient(0, 0, 0, H);
      g.addColorStop(0, rgba(col, 0.28));
      g.addColorStop(1, rgba(col, 0));
      ctx.beginPath();
      pts.forEach((q, i) => (i ? ctx.lineTo(q.x, q.y) : ctx.moveTo(q.x, q.y)));
      ctx.lineTo(pts[pts.length - 1].x, H);
      ctx.lineTo(pts[0].x, H);
      ctx.fillStyle = g;
      ctx.fill();
      ctx.beginPath();
      pts.forEach((q, i) => (i ? ctx.lineTo(q.x, q.y) : ctx.moveTo(q.x, q.y)));
      ctx.strokeStyle = col;
      ctx.lineWidth = 1.5;
      ctx.stroke();
    }
  }

  function toggleTunnel(tn) {
    if (tn.st === "off") tn.st = tn.id === "backup" ? "down" : "up";
    else tn.st = "off";
    tick();
    renderRows();
  }

  // ------------------------------------------------------------------ the map

  const map = { canvas: $("#map"), running: false, geo: null, stars: [], mouse: null };

  function layout(W, H) {
    const narrow = W < 640;
    const yH = narrow ? 110 : 150;
    const pad = narrow ? 44 : 110;
    const rtl = html.dir === "rtl";
    const xs = SERVERS.map((_, i) => pad + (i * (W - 2 * pad)) / (SERVERS.length - 1));
    const pos = {};
    SERVERS.forEach((s, i) => (pos[s.id] = rtl ? W - xs[i] : xs[i]));
    const base = yH + (narrow ? 52 : 64);
    const step = narrow ? 32 : 38;
    const chans = TUNNELS.map((tn) => {
      const y0 = base + tn.level * step;
      const p0 = { x: pos[tn.a], y: y0 };
      const p1 = { x: pos[tn.b], y: y0 + 16 };
      const c = { x: (p0.x + p1.x) / 2, y: y0 + 26 };
      const pts = Array.from({ length: 61 }, (_, i) => qb(p0, c, p1, i / 60));
      let len = 0;
      for (let i = 1; i < pts.length; i++) len += Math.hypot(pts[i].x - pts[i - 1].x, pts[i].y - pts[i - 1].y);
      return { tn, p0, p1, c, pts, len };
    });
    const depth = {};
    for (const ch of chans) {
      depth[ch.tn.a] = Math.max(depth[ch.tn.a] || 0, ch.p0.y);
      depth[ch.tn.b] = Math.max(depth[ch.tn.b] || 0, ch.p1.y);
    }
    const r = rng(11);
    map.stars = Array.from({ length: Math.round(W / 14) }, () => ({ x: r() * W, y: r() * (yH - 70) + 6, z: 0.3 + r() * 0.7, ph: r() * 6.28 }));
    const pebbles = Array.from({ length: Math.round(W / 9) }, () => ({ x: r() * W, y: yH + 10 + r() * (H - yH - 14), r: 0.6 + r() * 1.4 }));
    return { W, H, yH, pos, chans, depth, narrow, rtl, pebbles, key: `${W}x${H}${rtl}` };
  }

  function mapFrame(now) {
    if (!$("#app").classList.contains("is-on")) {
      map.running = false;
      return;
    }
    requestAnimationFrame(mapFrame);
    if (state.page !== "map") return;
    const { ctx, W, H } = fitCanvas(map.canvas);
    const rtl = html.dir === "rtl";
    if (!map.geo || map.geo.key !== `${W}x${H}${rtl}`) map.geo = layout(W, H);
    const g = map.geo;
    const time = now / 1000;
    const dt = Math.min(0.05, map.last ? time - map.last : 0.016);
    map.last = time;
    const still = state.low;

    ctx.clearRect(0, 0, W, H);

    // sky
    const sky = ctx.createLinearGradient(0, 0, 0, g.yH);
    sky.addColorStop(0, P.bg);
    sky.addColorStop(1, P.s1);
    ctx.fillStyle = sky;
    ctx.fillRect(0, 0, W, g.yH);
    const glow = ctx.createRadialGradient(W * (rtl ? 0.25 : 0.75), g.yH, 0, W * (rtl ? 0.25 : 0.75), g.yH, W * 0.5);
    glow.addColorStop(0, rgba(P.gold, P.night ? 0.12 : 0.22));
    glow.addColorStop(1, rgba(P.gold, 0));
    ctx.fillStyle = glow;
    ctx.fillRect(0, 0, W, g.yH);
    for (const s of map.stars) {
      const a = (P.night ? 0.7 : 0.2) * s.z * (still ? 0.8 : 0.6 + 0.4 * Math.sin(time * 1.3 + s.ph));
      ctx.fillStyle = P.night ? `rgba(233,240,255,${a})` : rgba(P.gold, a);
      ctx.fillRect(s.x, s.y, s.z * 1.6, s.z * 1.6);
    }

    // earth
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
    ctx.strokeStyle = rgba(P.gold, 0.06);
    ctx.lineWidth = 1;
    for (let i = 1; i < 6; i++) {
      const y = g.yH + i * ((H - g.yH) / 6);
      ctx.beginPath();
      ctx.moveTo(0, y);
      ctx.lineTo(W, y + 4);
      ctx.stroke();
    }

    // shafts
    ctx.lineCap = "round";
    for (const s of SERVERS) {
      ctx.strokeStyle = rgba(P.gold, 0.45);
      ctx.lineWidth = g.narrow ? 3 : 4;
      ctx.beginPath();
      ctx.moveTo(g.pos[s.id], g.yH);
      ctx.lineTo(g.pos[s.id], (g.depth[s.id] || g.yH + 40) + 4);
      ctx.stroke();
    }

    // channels
    const hover = map.hoverT || state.hot;
    for (const ch of g.chans) {
      const tn = ch.tn;
      const target = tn.st === "off" ? 0 : 1;
      const speed = still ? 10 : 1.1;
      tn.fill = tn.fill < target ? Math.min(target, tn.fill + dt * speed) : Math.max(target, tn.fill - dt * speed);
      const reach = tn.st === "down" ? 0.55 : 1; // a broken channel holds water up to the break
      const lit = hover === tn.id;
      const path = (from, to) => {
        const a = Math.floor(from * 60), b = Math.ceil(to * 60);
        ctx.beginPath();
        for (let i = a; i <= b; i++) (i === a ? ctx.moveTo : ctx.lineTo).call(ctx, ch.pts[i].x, ch.pts[i].y);
      };

      // bed
      ctx.lineWidth = g.narrow ? 12 : 16;
      ctx.strokeStyle = P.night ? "#06121f" : "#cdb489";
      path(0, 1);
      ctx.stroke();
      if (tn.st === "off" || tn.fill < 1) {
        ctx.save();
        ctx.setLineDash([4, 6]);
        ctx.lineWidth = 1.2;
        ctx.strokeStyle = rgba(P.text3, lit ? 0.9 : 0.55);
        path(0, 1);
        ctx.stroke();
        ctx.restore();
      }
      // water
      const w = Math.min(tn.fill, reach);
      if (w > 0.01) {
        const grad = ctx.createLinearGradient(ch.p0.x, 0, ch.p1.x, 0);
        grad.addColorStop(0, P.waterDeep);
        grad.addColorStop(1, lit ? P.waterBright : P.water);
        ctx.lineWidth = g.narrow ? 6 : 8;
        ctx.strokeStyle = grad;
        ctx.globalAlpha = still ? 0.35 + 0.65 * clamp((tn.down + tn.up) / 400, 0.2, 1) : 1;
        path(0, w);
        ctx.stroke();
        ctx.globalAlpha = 1;
      }
      // the break
      if (tn.st === "down" && tn.fill > 0.5) {
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

      // particles: downloads flow exit -> entry, uploads entry -> exit
      if (!still && tn.st === "up") {
        const rate = tn.down + tn.up;
        const want = Math.round(clamp(6 + rate / 9, 6, 56));
        while (tn.parts.length < want) tn.parts.push({ k: Math.random(), dir: Math.random() < 0.8 ? -1 : 1, s: 0.5 + Math.random() * 0.7, j: Math.random() * 6 });
        if (tn.parts.length > want) tn.parts.length = want;
        const v = clamp(50 + rate * 0.35, 50, 240) / ch.len; // in path fractions per second
        ctx.globalCompositeOperation = P.night ? "lighter" : "source-over";
        for (const q of tn.parts) {
          q.k += q.dir * v * dt * (q.dir < 0 ? 1 : 0.8);
          if (q.k < 0) q.k += 1;
          if (q.k > 1) q.k -= 1;
          if (q.k > tn.fill) continue;
          const i = q.k * 60, i0 = Math.floor(i), f = i - i0;
          const a = ch.pts[i0], b = ch.pts[Math.min(60, i0 + 1)];
          const x = lerp(a.x, b.x, f), y = lerp(a.y, b.y, f) + Math.sin(time * 3 + q.j) * 1.5;
          const r = (q.dir < 0 ? 7 : 5) * q.s * (g.narrow ? 0.8 : 1);
          ctx.globalAlpha = q.dir < 0 ? 1 : 0.6;
          ctx.drawImage(sprite, x - r, y - r, r * 2, r * 2);
        }
        ctx.globalAlpha = 1;
        ctx.globalCompositeOperation = "source-over";
      }
    }

    // channel labels
    ctx.textBaseline = "middle";
    ctx.direction = html.dir;
    for (const ch of g.chans) {
      const tn = ch.tn;
      const k = 0.28 + 0.14 * (tn.level % 3);
      const p = ch.pts[Math.round(k * 60)];
      let label = tn.id;
      if (tn.st === "up") {
        const r = fmtRate(tn.down + tn.up);
        label += `  ${r.v} ${r.u}`;
      } else label += `  ${stateLabel(tn.st)}`;
      ctx.font = uiFont(500, g.narrow ? 10 : 12);
      const tw = ctx.measureText(label).width + 14;
      const y = p.y - (g.narrow ? 12 : 15);
      ctx.fillStyle = rgba(P.night ? P.skyTop : "#fbf6ec", 0.85);
      roundRect(ctx, p.x - tw / 2, y - 9, tw, 18, 4);
      ctx.fill();
      ctx.strokeStyle = tn.st === "down" ? rgba(P.danger, 0.6) : rgba(P.gold, hover === tn.id ? 0.7 : 0.25);
      ctx.lineWidth = 1;
      ctx.stroke();
      ctx.fillStyle = tn.st === "down" ? P.danger : tn.st === "off" ? P.text3 : P.text;
      ctx.textAlign = "center";
      ctx.fillText(label, p.x, y + 1);
    }

    // horizon, mounds, server labels
    ctx.strokeStyle = P.gold;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(0, g.yH);
    ctx.lineTo(W, g.yH);
    ctx.stroke();
    SERVERS.forEach((s, i) => {
      const x = g.pos[s.id];
      const lit = map.hoverS === s.id;
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
      const lx = clamp(x, nw / 2 + 18, W - nw / 2 - 18); // keep edge labels on screen
      ctx.fillText(s.name, lx, ny);
      ctx.fillStyle = s.st === "up" ? P.water : s.st === "warn" ? P.warn : P.danger;
      ctx.beginPath();
      ctx.arc(lx - (nw / 2 + 8) * (rtl ? -1 : 1), ny, 3.5, 0, 6.28);
      ctx.fill();
      if (!g.narrow) {
        ctx.font = uiFont(400, 12);
        ctx.fillStyle = P.text3;
        ctx.fillText(`${s.loc[state.lang]} · ${t("role." + s.role)}`, lx, ny + 18);
      }
    });

    // keep the tooltip fresh while hovering
    if (map.mouse) updateTip();
  }

  function roundRect(ctx, x, y, w, h, r) {
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.arcTo(x + w, y, x + w, y + h, r);
    ctx.arcTo(x + w, y + h, x, y + h, r);
    ctx.arcTo(x, y + h, x, y, r);
    ctx.arcTo(x, y, x + w, y, r);
    ctx.closePath();
  }

  function hitTest(mx, my) {
    const g = map.geo;
    if (!g) return {};
    for (const s of SERVERS) {
      const x = g.pos[s.id];
      if (Math.abs(mx - x) < 40 && my > g.yH - 70 && my < g.yH + 8) return { s };
    }
    let best = null, bd = 14;
    for (const ch of g.chans) {
      for (const p of ch.pts) {
        const d = Math.hypot(p.x - mx, p.y - my);
        if (d < bd) (bd = d), (best = ch.tn);
      }
    }
    return { tn: best };
  }

  function updateTip() {
    const tip = $("#map-tip");
    const { x, y } = map.mouse;
    const hit = hitTest(x, y);
    map.hoverT = hit.tn ? hit.tn.id : null;
    map.hoverS = hit.s ? hit.s.id : null;
    map.canvas.classList.toggle("is-pointing", !!(hit.tn || hit.s));
    if (!hit.tn && !hit.s) {
      tip.classList.remove("is-on");
      return;
    }
    const row = (a, b, mono) => `<div class="row"><span>${a}</span><span class="${mono ? "mono" : "num"}">${b}</span></div>`;
    if (hit.tn) {
      const tn = hit.tn;
      const d = fmtRate(tn.down), u = fmtRate(tn.up);
      tip.innerHTML =
        `<b>${tn.id}</b><div class="row" style="margin-bottom:6px"><span class="mono">${byId(tn.a).name} → ${byId(tn.b).name}</span></div>` +
        (tn.st === "up"
          ? row(t("tip.down"), `${d.v} ${d.u}`) + row(t("tip.up"), `${u.v} ${u.u}`) + row(t("tip.lat"), `${fmt(tn.lat)} ms`) + row(t("tip.conns"), fmt(tn.open))
          : tn.st === "down"
          ? `<div class="row" style="color:var(--danger)"><span>${tn.err[state.lang]}</span></div>`
          : row(t("t.state"), stateLabel("off"))) +
        row(t("t.transport"), `${tn.tr} · ${tn.prof}`, true);
    } else {
      const s = hit.s;
      tip.innerHTML =
        `<b>${s.name}</b>` +
        row("IP", s.ip, true) +
        row(t("tip.role"), t("role." + s.role)) +
        row(t("tip.cpu"), `${fmt(s.cpu)}%`) +
        row(t("tip.ram"), `${fmt(s.ram)}%`) +
        row(t("tip.tunnels"), fmt(TUNNELS.filter((x) => x.a === s.id || x.b === s.id).length));
    }
    const band = map.canvas.getBoundingClientRect();
    const tw = tip.offsetWidth, th = tip.offsetHeight;
    let left = x + 16, top = y + 16;
    if (left + tw > band.width - 8) left = x - tw - 16;
    if (top + th > band.height - 8) top = y - th - 16;
    tip.style.left = `${Math.max(8, left)}px`;
    tip.style.top = `${Math.max(8, top)}px`;
    tip.classList.add("is-on");
  }

  map.canvas.addEventListener("pointermove", (e) => {
    const r = map.canvas.getBoundingClientRect();
    map.mouse = { x: e.clientX - r.left, y: e.clientY - r.top };
  });
  map.canvas.addEventListener("pointerleave", () => {
    map.mouse = null;
    map.hoverT = map.hoverS = null;
    $("#map-tip").classList.remove("is-on");
  });
  map.canvas.addEventListener("click", () => {
    if (map.hoverT) toast(`${t("toast.open")} ${map.hoverT}`);
  });

  function updateAlt() {
    $("#map-alt").textContent =
      t("alt.prefix") +
      " " +
      TUNNELS.map((tn) => `${tn.id} (${byId(tn.a).name} → ${byId(tn.b).name}): ${stateLabel(tn.st)}${tn.st === "up" ? ", " + fmtRate(tn.down + tn.up).v + " " + fmtRate(tn.down + tn.up).u : ""}`).join("; ");
  }

  // ------------------------------------------------------------------ command palette

  const commands = () => [
    { label: t("pal.new"), hint: "N", run: () => toast(t("toast.wizard")) },
    ...PAGES.map((p) => ({ label: `${t("pal.goto")} ${t("page." + p)}`, hint: "", run: () => setPage(p) })),
    { label: t("pal.speed"), hint: "", run: speedtest },
    { label: t("pal.theme"), hint: "", run: toggleTheme },
    { label: t("pal.lang"), hint: "", run: toggleLang },
    { label: t("pal.low"), hint: state.low ? "✓" : "", run: () => setLow(!state.low, true) },
    { label: t("pal.logout"), hint: "", run: logout },
  ];

  const pal = { el: $("#palette"), input: $("#palette-input"), list: $("#palette-list"), sel: 0, items: [] };

  function openPalette() {
    if (!$("#app").classList.contains("is-on")) return;
    pal.el.classList.add("is-on");
    pal.input.value = "";
    pal.sel = 0;
    renderPalette();
    setTimeout(() => pal.input.focus(), 30);
  }
  function closePalette() {
    pal.el.classList.remove("is-on");
  }
  function renderPalette() {
    const q = pal.input.value.trim().toLowerCase();
    pal.items = commands().filter((c) => c.label.toLowerCase().includes(q));
    pal.sel = clamp(pal.sel, 0, Math.max(0, pal.items.length - 1));
    pal.list.innerHTML = pal.items
      .map((c, i) => `<li role="option" aria-selected="${i === pal.sel}" data-i="${i}"><span>${c.label}</span><span class="hint">${c.hint}</span></li>`)
      .join("");
    for (const li of $$("li", pal.list)) {
      li.addEventListener("click", () => runPalette(Number(li.dataset.i)));
      li.addEventListener("mousemove", () => {
        if (pal.sel !== Number(li.dataset.i)) {
          pal.sel = Number(li.dataset.i);
          renderPalette();
        }
      });
    }
  }
  function runPalette(i) {
    const c = pal.items[i];
    closePalette();
    if (c) c.run();
  }
  pal.input.addEventListener("input", () => {
    pal.sel = 0;
    renderPalette();
  });
  pal.input.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") (pal.sel = (pal.sel + 1) % Math.max(1, pal.items.length)), renderPalette(), e.preventDefault();
    else if (e.key === "ArrowUp") (pal.sel = (pal.sel - 1 + pal.items.length) % Math.max(1, pal.items.length)), renderPalette(), e.preventDefault();
    else if (e.key === "Enter") runPalette(pal.sel);
  });
  pal.el.addEventListener("click", (e) => {
    if (e.target === pal.el) closePalette();
  });

  addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      pal.el.classList.contains("is-on") ? closePalette() : openPalette();
    } else if (e.key === "Escape") closePalette();
  });

  function speedtest() {
    toast(t("toast.speedRun"));
    const line = $("#loader-line");
    line.classList.add("is-on");
    const main = TUNNELS[0];
    const base = main.base;
    main.base = 900; // the channel runs full during the test
    setTimeout(() => {
      main.base = base;
      line.classList.remove("is-on");
      toast(t("toast.speedDone"));
    }, 4000);
  }

  // ------------------------------------------------------------------ toasts

  function toast(text) {
    const el = document.createElement("div");
    el.className = "toast";
    el.textContent = text;
    $("#toasts").append(el);
    setTimeout(() => {
      el.classList.add("is-leaving");
      setTimeout(() => el.remove(), 250);
    }, 3400);
  }

  // ------------------------------------------------------------------ wiring

  document.addEventListener("click", (e) => {
    const b = e.target.closest("[data-act]");
    if (!b) return;
    const act = b.dataset.act;
    if (act === "lang") toggleLang();
    else if (act === "theme") toggleTheme();
    else if (act === "lowpower") setLow(!state.low, true);
    else if (act === "palette") openPalette();
    else if (act === "logout") logout();
    else if (act === "replay") {
      closePalette();
      boot();
    }
  });

  reduced.addEventListener("change", () => setLow(reduced.matches, false));
  addEventListener("resize", () => drawSparks());

  readPalette();
  setLow(state.low, false);
  applyLang();
  tick();
  setInterval(tick, 1000);
  document.fonts?.ready.then(() => (map.geo = null));
  boot();
})();
