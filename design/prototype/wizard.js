/* Kariz panel prototype, step 9.3: the pair wizard. One tunnel = both sides, made at once. */
(() => {
  "use strict";
  const K = window.K;
  const { $, $$, t, fmt, clamp, rgba, fitCanvas, uiFont, SERVERS, TUNNELS, state, byId, toast } = K;
  const { icon, esc, tf, codeBlock, tomlHtml } = K;

  K.addStrings({
    fa: {
      "wz.title": "تانل جدید",
      "wz.editTitle": "ویرایش {name}",
      "wz.s1": "سرورها",
      "wz.s2": "ترنسپورت",
      "wz.s3": "اتصال",
      "wz.s4": "پورت‌ها",
      "wz.s5": "بازبینی",
      "wz.next": "بعدی",
      "wz.back": "قبلی",
      "wz.create": "ساختن تانل",
      "wz.save": "ذخیره و ری‌استارت",
      "wz.stepOf": "مرحلهٔ {n} از ۵",
      "wz.q1": "آب از کدام چاه به کدام چاه برود؟",
      "wz.l1": "کاربران به سرور entry وصل می‌شوند؛ exit به مقصدها می‌رسد. پنل هر دو طرف را با هم می‌سازد.",
      "wz.name": "نام تانل",
      "wz.nameHelp": "حروف لاتین، رقم، - و _ . نام سرویس روی هر دو سرور می‌شود: kariz@{name}",
      "wz.nameBad": "فقط حروف لاتین، رقم، - و _ (حداکثر ۳۲).",
      "wz.nameTaken": "تانلی با این نام هست.",
      "wz.entry": "entry: کاربران این‌جا وصل می‌شوند",
      "wz.exit": "exit: به مقصدها می‌رسد",
      "wz.same": "همان entry",
      "wz.mode": "چه کسی dial می‌کند؟",
      "wz.reverse": "reverse: exit به entry وصل می‌شود",
      "wz.direct": "direct: entry به exit وصل می‌شود",
      "wz.reverseHelp": "رایج‌ترین حالت وقتی entry در ایران است: entry فقط گوش می‌دهد و اتصالی به بیرون نمی‌زند.",
      "wz.directHelp": "لازم برای CDN: entry به لبهٔ CDN وصل می‌شود و CDN به exit.",
      "wz.q2": "آب از چه راهی برود؟",
      "wz.l2": "ترنسپورت روی هر دو طرف یکی است. هر کدام در یک چیز بهتر است.",
      "wz.profile": "پروفایل",
      "wz.rec": "پیشنهادی",
      "tr.tcp": "TCP ساده و سریع، بدون mux. برای مسیری که فیلتر نیست.",
      "tr.tcpmux": "TCP با mux: چند اتصال بلند، بی‌handshake برای هر کاربر.",
      "tr.ws": "WebSocket مثل یک وب‌اپ؛ از CDN بدون TLS هم رد می‌شود.",
      "tr.wss": "WebSocket روی TLS، از پشت CDN مثل Cloudflare. پنهان‌ترین.",
      "tr.quic": "روی UDP، سریع در مسیرهای پر از loss؛ datagram برای بازی.",
      "tr.kcp": "UDP با FEC؛ کمترین تأخیر برای بازی.",
      "trait.dpi": "پنهان‌کاری",
      "trait.speed": "سرعت",
      "trait.games": "بازی و UDP",
      "trait.cdn": "CDN",
      "pr.balanced": "پیش‌فرض‌های خوب برای بیشتر کارها.",
      "pr.ultraspeed": "بزرگ‌ترین پنجره‌ها و بافرها؛ حافظهٔ بیشتر.",
      "pr.gaming": "کمترین تأخیر: datagram، تکرار بسته، DSCP. بهترین با kcp.",
      "pr.gamingHint": "پروفایل gaming با kcp یا quic بهترین نتیجه را می‌دهد.",
      "wz.q3": "دو چاه کجا به هم می‌رسند؟",
      "wz.l3": "{listener} گوش می‌دهد و {dialer} به آن وصل می‌شود.",
      "wz.port": "پورت تانل روی {s}",
      "wz.portFree": "آزاد است",
      "wz.portUsed": "روی {s} در حال استفاده است ({by}).",
      "wz.portBad": "یک عدد بین ۱ و ۶۵۵۳۵.",
      "wz.bind": "روی چه آدرسی گوش بدهد؟",
      "wz.bindBoth": "IPv4 و IPv6",
      "wz.bindV4": "فقط IPv4",
      "wz.addr": "{dialer} با چه آدرسی به {listener} وصل شود؟",
      "wz.addrOther": "دامنه یا آدرس دیگر…",
      "wz.addrHelp": "برای wss پشت CDN، دامنه‌ای که روی CDN است را بنویسید.",
      "wz.wsPath": "مسیر WebSocket",
      "wz.wsPathBad": "باید با / شروع شود.",
      "wz.sentence": "{dialer} به {addr} وصل می‌شود.",
      "wz.q4": "کاربران به کدام پورت‌ها وصل شوند؟",
      "wz.l4": "هر قاعده: پروتکل، مقصد (آن‌طور که exit می‌بیند) و پورت‌ها. مثل kariz-manager: 443, 8080-8090, 2053=53",
      "wz.proto": "پروتکل",
      "wz.target": "میزبان مقصد روی exit",
      "wz.ports": "پورت‌ها",
      "wz.addRule": "قاعدهٔ دیگر",
      "wz.rmRule": "حذف قاعده",
      "wz.total": "{n} پورت در {r} قاعده",
      "wz.max": "حداکثر ۱۰۰۰ پورت در هر تانل.",
      "wz.pBad": "«{x}» قابل فهم نیست.",
      "wz.pRange": "«{x}»: پورت باید بین ۱ و ۶۵۵۳۵ باشد.",
      "wz.pRev": "«{x}»: اول بازه باید کوچک‌تر باشد.",
      "wz.pSize": "«{x}»: دو بازه هم‌اندازه نیستند.",
      "wz.pDup": "پورت {p} دو بار آمده است.",
      "wz.pUsed": "پورت {p} روی {s} در دست تانل {by} است.",
      "wz.pSys": "پورت {p} روی {s} در دست {by} است.",
      "wz.pEmpty": "دست‌کم یک پورت لازم است.",
      "wz.q5": "همه‌چیز درست است؟",
      "wz.l5": "این دو کانفیگ روی دو سرور نوشته می‌شوند. هر دو اول با kariz check بررسی می‌شوند.",
      "wz.route": "مسیر",
      "wz.how": "روش",
      "wz.conn": "اتصال",
      "wz.fw": "پورت‌ها",
      "wz.b1": "بررسی هر دو کانفیگ (kariz check)",
      "wz.b2": "نوشتن ⁦/etc/kariz/{name}.toml⁩ روی {s}",
      "wz.b4": "شروع ⁦kariz@{name}⁩ روی {s}",
      "wz.b6": "منتظر handshake",
      "wz.b7": "اولین استریم از داخل تانل",
      "wz.built": "تانل {name} جاری شد.",
      "wz.saved": "{name} ذخیره و ری‌استارت شد.",
      "wz.open": "باز کردن تانل",
      "wz.toMap": "دیدن روی نقشه",
      "wz.leave": "بستن؟ چیزی ساخته نمی‌شود.",
    },
    en: {
      "wz.title": "New tunnel",
      "wz.editTitle": "Edit {name}",
      "wz.s1": "Servers",
      "wz.s2": "Transport",
      "wz.s3": "Connection",
      "wz.s4": "Ports",
      "wz.s5": "Review",
      "wz.next": "Next",
      "wz.back": "Back",
      "wz.create": "Create the tunnel",
      "wz.save": "Save and restart",
      "wz.stepOf": "Step {n} of 5",
      "wz.q1": "Water from which well to which?",
      "wz.l1": "Users connect to the entry server; the exit reaches the targets. The panel sets up both sides at once.",
      "wz.name": "Tunnel name",
      "wz.nameHelp": "Latin letters, digits, - and _. Becomes the service on both servers: kariz@{name}",
      "wz.nameBad": "Latin letters, digits, - and _ only (32 at most).",
      "wz.nameTaken": "A tunnel has this name already.",
      "wz.entry": "entry: users connect here",
      "wz.exit": "exit: reaches the targets",
      "wz.same": "the entry",
      "wz.mode": "Who dials?",
      "wz.reverse": "reverse: the exit dials the entry",
      "wz.direct": "direct: the entry dials the exit",
      "wz.reverseHelp": "The usual choice when the entry is in Iran: it only listens and makes no outgoing connection.",
      "wz.directHelp": "Needed for a CDN: the entry dials the CDN edge, and the CDN reaches the exit.",
      "wz.q2": "Which way should the water take?",
      "wz.l2": "The transport is the same on both sides. Each is best at something.",
      "wz.profile": "Profile",
      "wz.rec": "recommended",
      "tr.tcp": "Plain TCP, fast, no mux. For a path that is not filtered.",
      "tr.tcpmux": "TCP with mux: a few long connections, no handshake per user.",
      "tr.ws": "WebSocket, looks like a web app; goes through a CDN without TLS too.",
      "tr.wss": "WebSocket over TLS, behind a CDN such as Cloudflare. The most hidden.",
      "tr.quic": "Over UDP, fast on lossy paths; datagrams for games.",
      "tr.kcp": "UDP with FEC; the lowest latency for games.",
      "trait.dpi": "Hidden",
      "trait.speed": "Speed",
      "trait.games": "Games, UDP",
      "trait.cdn": "CDN",
      "pr.balanced": "Good defaults for most uses.",
      "pr.ultraspeed": "The largest windows and buffers; more memory.",
      "pr.gaming": "Lowest latency: datagrams, packet copies, DSCP. Best with kcp.",
      "pr.gamingHint": "The gaming profile works best with kcp or quic.",
      "wz.q3": "Where do the two wells meet?",
      "wz.l3": "{listener} listens and {dialer} connects to it.",
      "wz.port": "Tunnel port on {s}",
      "wz.portFree": "free",
      "wz.portUsed": "in use on {s} ({by}).",
      "wz.portBad": "A number from 1 to 65535.",
      "wz.bind": "Listen on",
      "wz.bindBoth": "IPv4 and IPv6",
      "wz.bindV4": "IPv4 only",
      "wz.addr": "Which address should {dialer} use to reach {listener}?",
      "wz.addrOther": "A domain or another address…",
      "wz.addrHelp": "For wss behind a CDN, the domain that is on the CDN.",
      "wz.wsPath": "WebSocket path",
      "wz.wsPathBad": "Starts with /.",
      "wz.sentence": "{dialer} connects to {addr}.",
      "wz.q4": "Which ports do users connect to?",
      "wz.l4": "Each rule: a protocol, the target as the exit sees it, and ports. As in kariz-manager: 443, 8080-8090, 2053=53",
      "wz.proto": "Protocol",
      "wz.target": "Target host on the exit",
      "wz.ports": "Ports",
      "wz.addRule": "Another rule",
      "wz.rmRule": "Remove rule",
      "wz.total": "{n} ports in {r} rules",
      "wz.max": "At most 1,000 ports per tunnel.",
      "wz.pBad": "“{x}” is not understood.",
      "wz.pRange": "“{x}”: ports are 1 to 65535.",
      "wz.pRev": "“{x}”: a range goes low to high.",
      "wz.pSize": "“{x}”: the two ranges differ in size.",
      "wz.pDup": "Port {p} is listed twice.",
      "wz.pUsed": "Port {p} on {s} belongs to tunnel {by}.",
      "wz.pSys": "Port {p} on {s} is used by {by}.",
      "wz.pEmpty": "At least one port is needed.",
      "wz.q5": "Does it all look right?",
      "wz.l5": "These two configs are written to the two servers. Both are checked with kariz check first.",
      "wz.route": "Route",
      "wz.how": "How",
      "wz.conn": "Connection",
      "wz.fw": "Ports",
      "wz.b1": "Checking both configs (kariz check)",
      "wz.b2": "Writing /etc/kariz/{name}.toml on {s}",
      "wz.b4": "Starting kariz@{name} on {s}",
      "wz.b6": "Waiting for the handshake",
      "wz.b7": "The first stream through the tunnel",
      "wz.built": "Tunnel {name} is flowing.",
      "wz.saved": "{name} saved and restarted.",
      "wz.open": "Open the tunnel",
      "wz.toMap": "See it on the map",
      "wz.leave": "Close? Nothing is created.",
    },
  });

  const TRANSPORTS = {
    tcp: [1, 3, 1, 0],
    tcpmux: [2, 3, 2, 0],
    ws: [2, 2, 1, 1],
    wss: [3, 2, 1, 3],
    quic: [2, 3, 3, 0],
    kcp: [1, 2, 3, 0],
  };
  const PROFILES = ["balanced", "ultraspeed", "gaming"];
  const SYSTEM_PORTS = { 22: "sshd", 80: "nginx" };

  const el = $("#wizard");
  let W = null; // the wizard's state

  function fresh(edit) {
    if (edit) {
      return {
        edit,
        step: 0,
        name: edit.id,
        a: edit.a,
        b: edit.b,
        mode: edit.mode,
        tr: edit.tr,
        prof: edit.prof,
        port: String(edit.listenPort),
        v4: !!edit.ipv4only,
        addr: "",
        wsPath: edit.wsPath || "/ws",
        rules: groupRules(edit.fwd),
      };
    }
    const a = SERVERS.find((s) => s.role === "entry") || SERVERS[0];
    const b = SERVERS.find((s) => s.role === "exit" && s.id !== a.id) || SERVERS[1];
    const w = { step: 0, name: "", a: a.id, b: b.id, mode: "reverse", tr: "tcpmux", prof: "balanced", port: "", v4: false, addr: "", wsPath: "/ws", rules: [{ proto: "tcp", to: "127.0.0.1", ports: "" }] };
    w.port = String(freePort(w));
    return w;
  }

  function groupRules(fwd) {
    const rules = [];
    for (const [l, target, proto] of fwd) {
      const [host, tp] = splitHost(target);
      const spec = tp === l ? l : `${l}=${tp}`;
      const r = rules.find((x) => x.proto === proto && x.to === host);
      if (r) r.ports += `, ${spec}`;
      else rules.push({ proto, to: host, ports: spec });
    }
    return rules.length ? rules : [{ proto: "tcp", to: "127.0.0.1", ports: "" }];
  }
  const splitHost = (s) => {
    const i = s.lastIndexOf(":");
    return [s.slice(0, i), s.slice(i + 1)];
  };

  const listener = () => byId(W.mode === "reverse" ? W.a : W.b);
  const dialer = () => byId(W.mode === "reverse" ? W.b : W.a);

  // ports already taken on a server: other tunnels' listen ports and the system's
  function usedOn(serverId, skip) {
    const used = new Map();
    for (const [p, by] of Object.entries(SYSTEM_PORTS)) used.set(Number(p), { sys: by });
    for (const tn of TUNNELS) {
      if (tn === skip) continue;
      const lsn = tn.mode === "reverse" ? tn.a : tn.b;
      if (lsn === serverId) used.set(tn.listenPort, { by: tn.id });
      if (tn.a === serverId)
        for (const f of tn.fwd) {
          const [lo, hi] = f[0].split("-").map(Number);
          for (let p = lo; p <= (hi || lo); p++) used.set(p, { by: tn.id });
        }
    }
    return used;
  }
  function freePort(w) {
    const used = usedOn(w.mode === "reverse" ? w.a : w.b, w.edit);
    let p = 3080;
    while (used.has(p)) p++;
    return p;
  }

  // "443, 8080-8090, 2053=53, 3000-3005=4000-4005, 5000-5010=443"
  function parsePorts(text) {
    const out = [], errs = [];
    const range = (s) => {
      const m = s.match(/^(\d+)(?:-(\d+))?$/);
      if (!m) return null;
      const lo = Number(m[1]), hi = m[2] ? Number(m[2]) : lo;
      return { lo, hi };
    };
    for (const raw of text.split(",").map((x) => x.trim()).filter(Boolean)) {
      const [l, r] = raw.split(/[=:]/);
      const L = range(l.trim()), R = r === undefined ? L : range(r.trim());
      if (!L || !R) {
        errs.push(tf("wz.pBad", { x: raw }));
        continue;
      }
      if ([L.lo, L.hi, R.lo, R.hi].some((p) => p < 1 || p > 65535)) {
        errs.push(tf("wz.pRange", { x: raw }));
        continue;
      }
      if (L.lo > L.hi || R.lo > R.hi) {
        errs.push(tf("wz.pRev", { x: raw }));
        continue;
      }
      const n = L.hi - L.lo + 1, m = R.hi - R.lo + 1;
      if (m !== 1 && m !== n) {
        errs.push(tf("wz.pSize", { x: raw }));
        continue;
      }
      out.push({ raw, L, R, n });
    }
    return { out, errs };
  }

  // every rule parsed, with conflicts against each other and the entry server
  function checkRules() {
    const used = usedOn(W.a, W.edit);
    const seen = new Map();
    let total = 0;
    const res = W.rules.map((rule) => {
      const { out, errs } = parsePorts(rule.ports);
      for (const o of out) {
        total += o.n;
        for (let p = o.L.lo; p <= o.L.hi; p++) {
          const kinds = rule.proto === "tcp+udp" ? ["tcp", "udp"] : [rule.proto];
          for (const k of kinds) {
            const key = `${k}${p}`;
            if (seen.has(key)) {
              errs.push(tf("wz.pDup", { p: fmt(p) }));
              break;
            }
            seen.set(key, true);
          }
          const u = used.get(p);
          if (u && !errs.some((e) => e.includes(String(p)))) errs.push(u.sys ? tf("wz.pSys", { p, s: byId(W.a).name, by: u.sys }) : tf("wz.pUsed", { p, s: byId(W.a).name, by: u.by }));
          if (errs.length > 4) break;
        }
      }
      return { out, errs: [...new Set(errs)].slice(0, 4) };
    });
    return { res, total };
  }

  function valid(step) {
    if (step === 0) return nameError() === "" && W.a !== W.b;
    if (step === 2) return portState().ok && (!["ws", "wss"].includes(W.tr) || W.wsPath.startsWith("/"));
    if (step === 3) {
      const { res, total } = checkRules();
      return total > 0 && total <= 1000 && res.every((r) => !r.errs.length);
    }
    return true;
  }

  function nameError() {
    if (W.edit) return "";
    if (!W.name) return " ";
    if (!/^[A-Za-z0-9][A-Za-z0-9_-]{0,31}$/.test(W.name)) return t("wz.nameBad");
    if (TUNNELS.some((x) => x.id === W.name)) return t("wz.nameTaken");
    return "";
  }

  function portState() {
    const p = Number(W.port);
    if (!/^\d+$/.test(W.port) || p < 1 || p > 65535) return { ok: false, msg: t("wz.portBad") };
    const u = usedOn(listener().id, W.edit).get(p);
    if (u) return { ok: false, msg: tf("wz.portUsed", { s: listener().name, by: u.sys || `tunnel ${u.by}` }) };
    return { ok: true, msg: t("wz.portFree") };
  }

  // ------------------------------------------------------------------ open / close

  K.wizard = {
    open(opts = {}) {
      W = fresh(opts.edit);
      el.classList.add("is-on");
      render();
      setTimeout(() => $("input, button.pick, button.tile", $(".wz-body", el))?.focus(), 80);
    },
    close() {
      el.classList.remove("is-on");
      cancelAnimationFrame(build.raf);
      W = null;
    },
  };

  addEventListener("keydown", (e) => {
    if (!W || !el.classList.contains("is-on")) return;
    if (e.key === "Escape" && !$("#dialog").classList.contains("is-on")) K.wizard.close();
  });

  // ------------------------------------------------------------------ render

  function render(dirBack) {
    const steps = ["wz.s1", "wz.s2", "wz.s3", "wz.s4", "wz.s5"];
    const building = W.step === 5;
    el.innerHTML = `
      <div class="wz-top">
        <h2 id="wz-title">${W.edit ? tf("wz.editTitle", { name: W.edit.id }) : t("wz.title")}${W.name && !W.edit ? ` <span class="mono muted" style="font-size:var(--fs-md)">${esc(W.name)}</span>` : ""}</h2>
        <span class="muted small">${building ? "" : tf("wz.stepOf", { n: fmt(W.step + 1) })}</span>
        <button class="x-btn" id="wz-x" aria-label="${t("close")}">${icon("x")}</button>
      </div>
      <nav class="wz-steps" aria-label="steps">
        <div class="wz-channel"></div>
        <div class="wz-flow"><i style="width:${(Math.min(W.step, 4) / 4) * 100}%"></i></div>
        <ol>${steps.map((s, i) => `<li class="${i < W.step ? "done" : i === W.step ? "now" : ""}" data-go="${i}" ${i === W.step ? 'aria-current="step"' : ""}><span class="mound"></span><span class="lbl">${t(s)}</span></li>`).join("")}</ol>
      </nav>
      <div class="wz-body"><div class="wz-inner"><div class="wz-step ${dirBack ? "back" : ""}">${building ? buildHtml() : [step1, step2, step3, step4, step5][W.step]()}</div></div></div>
      ${
        building
          ? ""
          : `<div class="wz-foot">
        ${W.step ? `<button class="btn btn-ghost btn-sm" id="wz-back">${t("wz.back")}</button>` : ""}
        <div class="grow"></div>
        <span class="muted xs" id="wz-why"></span>
        <button class="btn btn-primary" id="wz-next" ${valid(W.step) ? "" : "disabled"}><span class="shine"></span>${W.step === 4 ? t(W.edit ? "wz.save" : "wz.create") : t("wz.next")}</button>
      </div>`
      }`;
    $("#wz-x", el).addEventListener("click", () => K.wizard.close());
    for (const li of $$(".wz-steps li.done", el)) li.addEventListener("click", () => go(Number(li.dataset.go)));
    $("#wz-back", el)?.addEventListener("click", () => go(W.step - 1));
    $("#wz-next", el)?.addEventListener("click", () => (W.step === 4 ? startBuild() : go(W.step + 1)));
    if (building) mountBuild();
    else [bind1, bind2, bind3, bind4, bind5][W.step]();
  }

  function go(step) {
    if (step > W.step && !valid(W.step)) return;
    const back = step < W.step;
    W.step = step;
    render(back);
    $(".wz-body", el).scrollTop = 0;
  }

  const refreshNext = () => {
    const b = $("#wz-next", el);
    if (b) b.disabled = !valid(W.step);
  };

  // ------------------------------------------------------------------ 1. servers

  function pickList(role) {
    const cur = role === "a" ? W.a : W.b;
    const other = role === "a" ? W.b : W.a;
    return SERVERS.map((s) => {
      const off = s.id === other && role === "b";
      return `<button class="pick" role="radio" data-pick="${role}" data-id="${s.id}" aria-checked="${s.id === cur}" ${off ? 'aria-disabled="true"' : ""}>
        <span class="radio"></span>
        <span><span class="nm">${s.name}</span><br><span class="mt">${s.loc[state.lang]} · <span class="mono" dir="ltr">${s.ip}</span></span></span>
        <span class="mt">${off ? t("wz.same") : ""}</span></button>`;
    }).join("");
  }

  function step1() {
    const rev = W.mode === "reverse";
    return `<div><h3 class="wz-q">${t("wz.q1")}</h3><p class="wz-lead">${t("wz.l1")}</p></div>
      <div class="field" style="max-width:420px"><label for="wz-name">${t("wz.name")}</label>
        <input class="text mono" id="wz-name" dir="ltr" autocomplete="off" placeholder="main" value="${esc(W.name)}" ${W.edit ? "disabled" : ""}>
        <span class="err" id="wz-name-err"></span><span class="help">${tf("wz.nameHelp", { name: esc(W.name || "name") })}</span></div>
      <div class="pair">
        <div class="field"><span class="label">${t("wz.entry")}</span><div class="pick-list" role="radiogroup">${pickList("a")}</div></div>
        <div class="pair-mid" aria-hidden="true">
          <svg viewBox="0 0 90 36"><path d="M6 18h78" stroke="var(--stratum-3)" stroke-width="8" stroke-linecap="round"/><path d="M6 18h78" stroke="var(--water)" stroke-width="4" stroke-linecap="round" stroke-dasharray="6 7"><animate attributeName="stroke-dashoffset" from="${rev ? "0" : "26"}" to="${rev ? "26" : "0"}" dur="1s" repeatCount="indefinite"/></path>
          <path d="${rev ? "M16 10l-10 8 10 8" : "M74 10l10 8-10 8"}" fill="none" stroke="var(--accent)" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
          <span>${rev ? "exit → entry" : "entry → exit"}</span>
        </div>
        <div class="field"><span class="label">${t("wz.exit")}</span><div class="pick-list" role="radiogroup">${pickList("b")}</div></div>
      </div>
      <div class="field"><span class="label">${t("wz.mode")}</span>
        <div class="seg" role="group" style="justify-self:start;flex-wrap:wrap"><button data-mode="reverse" aria-pressed="${rev}">${t("wz.reverse")}</button><button data-mode="direct" aria-pressed="${!rev}">${t("wz.direct")}</button></div>
        <span class="help">${t(rev ? "wz.reverseHelp" : "wz.directHelp")}</span></div>`;
  }
  function bind1() {
    const name = $("#wz-name", el);
    const showErr = (force) => {
      const e = nameError();
      $("#wz-name-err", el).textContent = e.trim() && (force || W.name) ? e : "";
      name.setAttribute("aria-invalid", String(!!e.trim() && !!W.name));
    };
    name.addEventListener("input", () => {
      W.name = name.value.trim();
      showErr();
      refreshNext();
      $(".field .help", el).textContent = tf("wz.nameHelp", { name: W.name || "name" });
    });
    for (const p of $$("[data-pick]", el))
      p.addEventListener("click", () => {
        if (p.getAttribute("aria-disabled") === "true") return;
        if (p.dataset.pick === "a") {
          W.a = p.dataset.id;
          if (W.b === W.a) W.b = SERVERS.find((s) => s.id !== W.a && s.role === "exit")?.id || SERVERS.find((s) => s.id !== W.a).id;
        } else W.b = p.dataset.id;
        W.port = String(freePort(W));
        render();
      });
    for (const b of $$("[data-mode]", el))
      b.addEventListener("click", () => {
        W.mode = b.dataset.mode;
        W.port = String(freePort(W));
        render();
      });
  }

  // ------------------------------------------------------------------ 2. transport

  function step2() {
    const traits = ["trait.dpi", "trait.speed", "trait.games", "trait.cdn"];
    return `<div><h3 class="wz-q">${t("wz.q2")}</h3><p class="wz-lead">${t("wz.l2")}</p></div>
      <div class="tiles" role="radiogroup" aria-label="transport">${Object.entries(TRANSPORTS)
        .map(
          ([tr, v]) => `<button class="tile" role="radio" data-tr="${tr}" aria-checked="${W.tr === tr}">
            <span class="tick">${icon("check")}</span>
            <span class="t1"><span class="mono">${tr}</span>${tr === "tcpmux" ? `<span class="badge">${t("wz.rec")}</span>` : ""}</span>
            <span class="t2">${t("tr." + tr)}</span>
            <span class="traits">${traits.map((k, i) => `<span class="trait"><span>${t(k)}</span><span class="dots">${[0, 1, 2].map((d) => `<i class="${d < v[i] ? "on" : ""}"></i>`).join("")}</span></span>`).join("")}</span>
          </button>`
        )
        .join("")}</div>
      <div class="field"><span class="label">${t("wz.profile")}</span>
      <div class="tiles" role="radiogroup" aria-label="${t("wz.profile")}">${PROFILES.map(
        (p) => `<button class="tile" role="radio" data-pr="${p}" aria-checked="${W.prof === p}"><span class="tick">${icon("check")}</span><span class="t1 mono">${p}</span><span class="t2">${t("pr." + p)}</span></button>`
      ).join("")}</div>
      <span class="help" id="wz-pr-hint">${W.prof === "gaming" && !["kcp", "quic"].includes(W.tr) ? t("pr.gamingHint") : ""}</span></div>`;
  }
  function bind2() {
    for (const b of $$("[data-tr]", el))
      b.addEventListener("click", () => {
        W.tr = b.dataset.tr;
        for (const x of $$("[data-tr]", el)) x.setAttribute("aria-checked", String(x === b));
        $("#wz-pr-hint", el).textContent = W.prof === "gaming" && !["kcp", "quic"].includes(W.tr) ? t("pr.gamingHint") : "";
      });
    for (const b of $$("[data-pr]", el))
      b.addEventListener("click", () => {
        W.prof = b.dataset.pr;
        for (const x of $$("[data-pr]", el)) x.setAttribute("aria-checked", String(x === b));
        $("#wz-pr-hint", el).textContent = W.prof === "gaming" && !["kcp", "quic"].includes(W.tr) ? t("pr.gamingHint") : "";
      });
  }

  // ------------------------------------------------------------------ 3. connection

  const addrOf = () => {
    const l = listener();
    const host = W.addr || (l.ip.includes(":") ? `[${l.ip}]` : l.ip);
    return `${host}:${W.port}`;
  };

  function step3() {
    const l = listener(), d = dialer();
    const ps = portState();
    const ws = ["ws", "wss"].includes(W.tr);
    return `<div><h3 class="wz-q">${t("wz.q3")}</h3><p class="wz-lead">${tf("wz.l3", { listener: `<b>${l.name}</b>`, dialer: `<b>${d.name}</b>` })}</p></div>
      <div class="grid-2">
        <div class="field"><label for="wz-port">${tf("wz.port", { s: l.name })}</label>
          <input class="text mono" id="wz-port" inputmode="numeric" dir="ltr" value="${esc(W.port)}" aria-invalid="${!ps.ok}">
          <span class="${ps.ok ? "ok" : "err"}" id="wz-port-msg">${ps.ok ? "✓ " : ""}${ps.msg}</span></div>
        <div class="field"><span class="label">${t("wz.bind")}</span>
          <div class="seg" role="group" style="justify-self:start"><button data-v4="0" aria-pressed="${!W.v4}">${t("wz.bindBoth")}</button><button data-v4="1" aria-pressed="${W.v4}">${t("wz.bindV4")}</button></div>
          <span class="help mono" dir="ltr">${W.v4 ? "0.0.0.0" : "[::]"}:${esc(W.port)}</span></div>
      </div>
      <div class="field"><label for="wz-addr">${tf("wz.addr", { dialer: d.name, listener: l.name })}</label>
        <select class="select" id="wz-addr-sel" style="max-width:420px"><option value="">${l.ip}</option><option value="__other" ${W.addr ? "selected" : ""}>${t("wz.addrOther")}</option></select>
        <input class="text mono" id="wz-addr" dir="ltr" placeholder="tunnel.example.com" value="${esc(W.addr)}" style="max-width:420px" ${W.addr ? "" : "hidden"}>
        <span class="help">${W.tr === "wss" ? t("wz.addrHelp") : ""}</span></div>
      ${ws ? `<div class="field" style="max-width:420px"><label for="wz-ws">${t("wz.wsPath")}</label><input class="text mono" id="wz-ws" dir="ltr" value="${esc(W.wsPath)}"><span class="err" id="wz-ws-err"></span></div>` : ""}
      <div class="waiting done" style="border-style:dashed;background:transparent"><span id="wz-sentence">${tf("wz.sentence", { dialer: `<b>${d.name}</b>`, addr: `<span class="mono" dir="ltr">${esc(addrOf())}</span>` })}</span></div>`;
  }
  function bind3() {
    const upd = () => {
      const ps = portState();
      const m = $("#wz-port-msg", el);
      m.className = ps.ok ? "ok" : "err";
      m.textContent = (ps.ok ? "✓ " : "") + ps.msg;
      $("#wz-port", el).setAttribute("aria-invalid", String(!ps.ok));
      $("#wz-sentence", el).innerHTML = tf("wz.sentence", { dialer: `<b>${dialer().name}</b>`, addr: `<span class="mono" dir="ltr">${esc(addrOf())}</span>` });
      refreshNext();
    };
    $("#wz-port", el).addEventListener("input", (e) => {
      W.port = e.target.value.trim();
      upd();
    });
    for (const b of $$("[data-v4]", el))
      b.addEventListener("click", () => {
        W.v4 = b.dataset.v4 === "1";
        render();
      });
    $("#wz-addr-sel", el).addEventListener("change", (e) => {
      const inp = $("#wz-addr", el);
      inp.hidden = e.target.value !== "__other";
      if (inp.hidden) W.addr = "";
      else inp.focus();
      upd();
    });
    $("#wz-addr", el).addEventListener("input", (e) => {
      W.addr = e.target.value.trim();
      upd();
    });
    $("#wz-ws", el)?.addEventListener("input", (e) => {
      W.wsPath = e.target.value.trim();
      $("#wz-ws-err", el).textContent = W.wsPath.startsWith("/") ? "" : t("wz.wsPathBad");
      refreshNext();
    });
  }

  // ------------------------------------------------------------------ 4. ports

  function step4() {
    return `<div><h3 class="wz-q">${t("wz.q4")}</h3><p class="wz-lead">${t("wz.l4")}</p></div>
      <div id="wz-rules" style="display:grid;gap:var(--sp-5)">${W.rules.map(ruleHtml).join("")}</div>
      <div style="display:flex;align-items:center;gap:var(--sp-4);flex-wrap:wrap">
        <button class="btn btn-ghost btn-sm" id="wz-add-rule">${icon("plus")}${t("wz.addRule")}</button>
        <span class="muted small" id="wz-total"></span></div>`;
  }
  function ruleHtml(r, i) {
    return `<div class="review-card" data-rule="${i}" style="display:grid;gap:var(--sp-3)">
      <div style="display:flex;gap:var(--sp-3);flex-wrap:wrap;align-items:end">
        <div class="field"><span class="label">${t("wz.proto")}</span><div class="seg" role="group">${["tcp", "udp", "tcp+udp"].map((p) => `<button data-proto="${p}" aria-pressed="${r.proto === p}">${p}</button>`).join("")}</div></div>
        <div class="field" style="flex:1;min-width:180px"><label for="wz-to-${i}">${t("wz.target")}</label><input class="text mono" id="wz-to-${i}" data-to dir="ltr" value="${esc(r.to)}"></div>
        ${W.rules.length > 1 ? `<button class="x-btn" data-rm aria-label="${t("wz.rmRule")}">${icon("trash")}</button>` : ""}
      </div>
      <div class="field"><label for="wz-ports-${i}">${t("wz.ports")}</label><input class="text mono" id="wz-ports-${i}" data-ports dir="ltr" placeholder="443, 8080-8090, 2053=53" value="${esc(r.ports)}" autocomplete="off"></div>
      <div class="port-chips" data-chips></div><div class="err" data-errs></div></div>`;
  }
  function bind4() {
    const paint = () => {
      const { res, total } = checkRules();
      res.forEach((r, i) => {
        const card = $(`[data-rule="${i}"]`, el);
        if (!card) return;
        const rule = W.rules[i];
        const chips = $("[data-chips]", card);
        const html = r.out
          .map((o) => {
            const tgt = o.R.lo === o.R.hi && o.n > 1 ? `${o.R.lo}` : o.R.lo === o.L.lo ? "" : o.R.lo === o.R.hi ? `${o.R.lo}` : `${o.R.lo}-${o.R.hi}`;
            const lst = o.L.lo === o.L.hi ? `${o.L.lo}` : `${o.L.lo}-${o.L.hi}`;
            return `<span class="pchip">:${lst} → ${esc(rule.to)}:${tgt || lst}${o.n > 1 ? ` <small>×${o.n}</small>` : ""}</span>`;
          })
          .join("");
        if (chips.dataset.html !== html) {
          chips.innerHTML = html;
          chips.dataset.html = html;
        }
        $("[data-errs]", card).innerHTML = r.errs.map(esc).join("<br>");
        $("[data-ports]", card).setAttribute("aria-invalid", String(!!r.errs.length));
      });
      $("#wz-total", el).innerHTML = total > 1000 ? `<span style="color:var(--danger)">${t("wz.max")}</span>` : total ? tf("wz.total", { n: fmt(total), r: fmt(W.rules.length) }) : t("wz.pEmpty");
      refreshNext();
    };
    $$("[data-rule]", el).forEach((card) => {
      const i = Number(card.dataset.rule);
      $("[data-ports]", card).addEventListener("input", (e) => {
        W.rules[i].ports = e.target.value;
        paint();
      });
      $("[data-to]", card).addEventListener("input", (e) => {
        W.rules[i].to = e.target.value.trim() || "127.0.0.1";
        paint();
      });
      for (const b of $$("[data-proto]", card))
        b.addEventListener("click", () => {
          W.rules[i].proto = b.dataset.proto;
          for (const x of $$("[data-proto]", card)) x.setAttribute("aria-pressed", String(x === b));
          paint();
        });
      $("[data-rm]", card)?.addEventListener("click", () => {
        W.rules.splice(i, 1);
        render();
      });
    });
    $("#wz-add-rule", el).addEventListener("click", () => {
      W.rules.push({ proto: "udp", to: "127.0.0.1", ports: "" });
      render();
      $(`[data-rule="${W.rules.length - 1}"] [data-ports]`, el)?.focus();
    });
    paint();
  }

  // ------------------------------------------------------------------ 5. review

  function draft() {
    const fwd = [];
    for (const r of W.rules)
      for (const o of parsePorts(r.ports).out) {
        const l = o.L.lo === o.L.hi ? `${o.L.lo}` : `${o.L.lo}-${o.L.hi}`;
        const tg = o.R.lo === o.R.hi ? `${o.R.lo}` : `${o.R.lo}-${o.R.hi}`;
        fwd.push([l, `${r.to}:${tg}`, r.proto]);
      }
    return {
      id: W.edit ? W.edit.id : W.name,
      a: W.a,
      b: W.b,
      mode: W.mode,
      tr: W.tr,
      prof: W.prof,
      listenPort: Number(W.port),
      ipv4only: W.v4,
      wsPath: W.wsPath,
      fwd,
      token: W.edit ? W.edit.token : "kz_" + Array.from(crypto.getRandomValues(new Uint8Array(16)), (b) => b.toString(16).padStart(2, "0")).join(""),
    };
  }

  function step5() {
    const d = draft();
    const cf = K.configs(d);
    const a = byId(d.a), b = byId(d.b);
    const n = d.fwd.reduce((s, f) => s + K.portCount(f[0]), 0);
    return `<div><h3 class="wz-q">${t("wz.q5")}</h3><p class="wz-lead">${t("wz.l5")}</p></div>
      <div class="review">
        <div class="review-card"><h4>${t("wz.route")}</h4><b>${a.name}</b> → <b>${b.name}</b><div class="muted small">${d.mode}</div></div>
        <div class="review-card"><h4>${t("wz.how")}</h4><span class="tag">${d.tr}</span> <span class="tag">${d.prof}</span></div>
        <div class="review-card"><h4>${t("wz.conn")}</h4><span class="mono small" dir="ltr">${esc(dialer().name)} → ${esc(addrOf())}</span>${["ws", "wss"].includes(d.tr) ? `<div class="mono small muted" dir="ltr">path ${esc(d.wsPath)}</div>` : ""}</div>
        <div class="review-card"><h4>${t("wz.fw")}</h4>${tf("wz.total", { n: fmt(n), r: fmt(W.rules.length) })}<div class="port-chips" style="margin-top:6px">${d.fwd.slice(0, 8).map((f) => `<span class="pchip">:${f[0]} → ${esc(f[1])}</span>`).join("")}${d.fwd.length > 8 ? `<span class="pchip">+${d.fwd.length - 8}</span>` : ""}</div></div>
      </div>
      <div><div class="tabs" role="tablist"><button role="tab" data-side="entry" aria-selected="true">${t("d.entrySide")} · ${a.name}</button><button role="tab" data-side="exit" aria-selected="false">${t("d.exitSide")} · ${b.name}</button></div>
      <div id="wz-conf">${codeBlock(tomlHtml(cf.entry), cf.entry)}</div></div>`;
  }
  function bind5() {
    const d = draft();
    const cf = K.configs(d);
    for (const b of $$("[data-side]", el))
      b.addEventListener("click", () => {
        for (const x of $$("[data-side]", el)) x.setAttribute("aria-selected", String(x === b));
        $("#wz-conf", el).innerHTML = codeBlock(tomlHtml(cf[b.dataset.side]), cf[b.dataset.side]);
      });
  }

  // ------------------------------------------------------------------ the build

  const build = { raf: 0, dig: 0, fill: 0, t0: 0, parts: [] };

  function buildHtml() {
    const d = W.draft;
    const a = byId(d.a), b = byId(d.b);
    const lsn = d.mode === "reverse" ? a : b, dl = d.mode === "reverse" ? b : a;
    const items = W.edit
      ? [t("wz.b1"), tf("wz.b2", { name: d.id, s: a.name }), tf("wz.b2", { name: d.id, s: b.name }), tf("wz.b4", { name: d.id, s: lsn.name }), tf("wz.b4", { name: d.id, s: dl.name }), t("wz.b6")]
      : [t("wz.b1"), tf("wz.b2", { name: d.id, s: a.name }), tf("wz.b2", { name: d.id, s: b.name }), tf("wz.b4", { name: d.id, s: lsn.name }), tf("wz.b4", { name: d.id, s: dl.name }), t("wz.b6"), t("wz.b7")];
    return `<div class="build">
      <canvas id="wz-build" aria-hidden="true"></canvas>
      <div><h3 class="wz-q" id="wz-build-title">${esc(d.id)}</h3>
        <ol class="checklist" id="wz-check">${items.map((x) => `<li><span class="st"></span><span>${esc(x)}</span><time></time></li>`).join("")}</ol>
        <div class="row-actions" id="wz-done" style="margin-top:var(--sp-5)" hidden>
          <button class="btn btn-primary" id="wz-open"><span class="shine"></span>${t("wz.open")}</button>
          <button class="btn btn-ghost" id="wz-map">${t("wz.toMap")}</button></div>
      </div></div>`;
  }

  function startBuild() {
    W.draft = draft();
    W.step = 5;
    render();
  }

  function mountBuild() {
    const items = $$("#wz-check li", el);
    build.dig = 0;
    build.fill = 0;
    build.parts = [];
    build.t0 = performance.now();
    let i = 0;
    const next = () => {
      if (!W) return;
      if (i > 0) {
        const prev = items[i - 1];
        prev.className = "ok";
        $(".st", prev).innerHTML = icon("check");
        $("time", prev).textContent = `${fmt(0.2 + Math.random() * 0.6, 1)} s`;
      }
      if (i === items.length) return finish();
      items[i].className = "run";
      // the channel digs while the files are written and fills when both sides run
      build.digTo = clamp((i + 1) / (items.length - 2), 0, 1);
      if (i >= items.length - 2) build.fillOn = true;
      i++;
      setTimeout(next, state.low ? 200 : 520 + Math.random() * 380);
    };
    build.fillOn = false;
    build.digTo = 0;
    next();
    cancelAnimationFrame(build.raf);
    const loop = (now) => {
      if (!W || W.step !== 5) return;
      drawBuild(now);
      build.raf = requestAnimationFrame(loop);
    };
    build.raf = requestAnimationFrame(loop);
  }

  function finish() {
    const d = W.draft;
    let tn;
    if (W.edit) {
      tn = W.edit;
      Object.assign(tn, d);
      tn.ports = d.fwd.reduce((s, f) => s + K.portCount(f[0]), 0);
      tn.fill = 0;
      toast(tf("wz.saved", { name: tn.id }));
    } else {
      tn = Object.assign(d, {
        st: "up",
        base: 60 + Math.round(Math.random() * 120),
        conns: 60 + Math.round(Math.random() * 80),
        lat: 35 + Math.round(Math.random() * 20),
        level: Math.max(-1, ...TUNNELS.map((x) => x.level)) + 1,
        fill: 0,
        noise: Math.random(),
        down: 0,
        up: 0,
        open: 0,
        hist: Array.from({ length: 60 }, () => 0),
        parts: [],
        uptime: 1,
        events: [{ at: Date.now(), k: "ev.session", cls: "" }, { at: Date.now() - 2000, k: "ev.start", cls: "" }],
      });
      tn.ports = tn.fwd.reduce((s, f) => s + K.portCount(f[0]), 0);
      TUNNELS.push(tn);
      K.seedHistory(tn);
      tn.h.down.fill(0);
      tn.h.up.fill(0);
      toast(tf("wz.built", { name: tn.id }));
    }
    K.tick();
    K.renderRows();
    const done = $("#wz-done", el);
    done.hidden = false;
    $("#wz-open", el).addEventListener("click", () => {
      K.wizard.close();
      K.setPage("tunnel", false, tn.id);
    });
    $("#wz-map", el).addEventListener("click", () => {
      K.wizard.close();
      K.setPage("map");
    });
    $("#wz-open", el).focus();
  }

  function drawBuild(now) {
    const cv = $("#wz-build", el);
    if (!cv) return;
    const P = K.P;
    const { ctx, W: w, H } = fitCanvas(cv);
    const rtl = document.documentElement.dir === "rtl";
    const time = now / 1000;
    const d = W.draft;
    build.dig += (build.digTo - build.dig) * 0.06;
    if (build.fillOn) build.fill = Math.min(1, build.fill + 0.012);
    const yH = 60, xa = rtl ? w - 50 : 50, xb = rtl ? 50 : w - 50;
    const pt = (k) => ({ x: xa + (xb - xa) * k, y: 132 + 16 * k + Math.sin(k * Math.PI) * 14 });
    ctx.clearRect(0, 0, w, H);
    const g = ctx.createLinearGradient(0, yH, 0, H);
    g.addColorStop(0, P.earth);
    g.addColorStop(1, P.bg);
    ctx.fillStyle = g;
    ctx.fillRect(0, yH, w, H - yH);
    // shafts go down first
    ctx.strokeStyle = rgba(P.gold, 0.55);
    ctx.lineWidth = 4;
    ctx.lineCap = "round";
    const shaft = clamp(build.dig * 3, 0, 1);
    for (const [x, k] of [[xa, 0], [xb, 1]]) {
      ctx.beginPath();
      ctx.moveTo(x, yH);
      ctx.lineTo(x, yH + (pt(k).y - yH) * shaft);
      ctx.stroke();
    }
    const stroke = (from, to, width, col, dash) => {
      if (to <= from) return;
      ctx.save();
      if (dash) ctx.setLineDash(dash);
      ctx.strokeStyle = col;
      ctx.lineWidth = width;
      ctx.beginPath();
      for (let i = 0; i <= 60; i++) {
        const p = pt(from + ((to - from) * i) / 60);
        i ? ctx.lineTo(p.x, p.y) : ctx.moveTo(p.x, p.y);
      }
      ctx.stroke();
      ctx.restore();
    };
    // the channel is dug from both ends towards the middle
    const dig = clamp((build.dig - 0.2) / 0.8, 0, 1) / 2;
    stroke(0, dig, 18, P.night ? "#06121f" : "#cdb489");
    stroke(1 - dig, 1, 18, P.night ? "#06121f" : "#cdb489");
    if (dig < 0.5) stroke(dig, 1 - dig, 1.2, rgba(P.text3, 0.5), [4, 6]);
    // then the water runs through
    if (build.fill > 0) {
      const wg = ctx.createLinearGradient(xa, 0, xb, 0);
      wg.addColorStop(0, P.waterDeep);
      wg.addColorStop(1, P.waterBright);
      stroke(0, build.fill, 10, wg);
      if (!state.low) {
        while (build.parts.length < 40) build.parts.push({ k: Math.random(), s: 0.5 + Math.random() * 0.7 });
        ctx.fillStyle = P.waterBright;
        for (const q of build.parts) {
          q.k = (q.k + 0.004 * (1 + q.s)) % 1;
          if (q.k > build.fill) continue;
          const p = pt(q.k);
          ctx.globalAlpha = 0.9;
          ctx.beginPath();
          ctx.arc(p.x, p.y + Math.sin(time * 4 + q.s * 9) * 2, 2.4 * q.s, 0, 6.28);
          ctx.fill();
        }
        ctx.globalAlpha = 1;
      }
      if (build.fill >= 1) {
        const glow = ctx.createRadialGradient(pt(0.5).x, pt(0.5).y, 0, pt(0.5).x, pt(0.5).y, 120);
        glow.addColorStop(0, rgba(P.waterBright, 0.18 + 0.06 * Math.sin(time * 2)));
        glow.addColorStop(1, rgba(P.waterBright, 0));
        ctx.fillStyle = glow;
        ctx.fillRect(0, yH, w, H - yH);
      }
    }
    // horizon, mounds, names
    ctx.strokeStyle = P.gold;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.moveTo(0, yH);
    ctx.lineTo(w, yH);
    ctx.stroke();
    ctx.fillStyle = P.gold;
    for (const x of [xa, xb]) {
      ctx.beginPath();
      ctx.ellipse(x, yH, 18, 12, 0, Math.PI, 0);
      ctx.fill();
    }
    ctx.textAlign = "center";
    ctx.font = uiFont(600, 13);
    ctx.fillStyle = P.text;
    ctx.fillText(byId(d.a).name, xa, yH - 26);
    ctx.fillText(byId(d.b).name, xb, yH - 26);
  }
})();
