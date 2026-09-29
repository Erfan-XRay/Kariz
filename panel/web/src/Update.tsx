import { useEffect, useRef, useState } from "react";
import { ApiError, api } from "./api";
import type { UpdateStatus } from "./api";
import { Checklist, describeError, useOp } from "./ops";
import { useApp } from "./store";
import { Dialog, Seg } from "./ui";

/** The update status of the panel, kept fresh (a look every minute is cheap: it is in memory). */
export function useUpdate(enabled: boolean): { status: UpdateStatus | null; reload: () => void } {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    const load = () =>
      api
        .update()
        .then((s) => alive && setStatus(s))
        .catch(() => {});
    void load();
    const id = setInterval(load, 60000);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [enabled, tick]);
  return { status, reload: () => setTick((n) => n + 1) };
}

/** A small notice for the top bar: a newer release exists. */
export function UpdatePill({ status, onOpen, onServers }: { status: UpdateStatus | null; onOpen: () => void; onServers: () => void }) {
  const { t, num } = useApp();
  if (!status) return null;
  if (status.state === "newer" && status.latest) {
    return (
      <button className="update-pill" type="button" onClick={onOpen}>
        <i aria-hidden="true" />
        {t("upd.pill", { v: status.latest.version })}
      </button>
    );
  }
  if (status.outdated.length > 0) {
    return (
      <button className="update-pill" type="button" onClick={onServers}>
        <i aria-hidden="true" />
        {t("upd.pillServers", { n: num(status.outdated.length) })}
      </button>
    );
  }
  return null;
}

/** Updates the servers whose agent is behind the panel, one at a time. */
export function ServersDialog({ status, onClose }: { status: UpdateStatus; onClose: () => void }) {
  const { t } = useApp();
  const [restart, setRestart] = useState(true);
  const [id, setId] = useState<string | null>(null);
  const [error, setError] = useState("");
  const op = useOp(id);
  const names = new Map(status.outdated.map((s) => [s.id, s.name]));
  const running = !!id && (!op || op.state === "running");
  const go = async () => {
    setError("");
    try {
      setId((await api.updateServers(restart)).op);
    } catch (e) {
      setError(e instanceof ApiError ? describeError(t, e.code) : String(e));
    }
  };
  return (
    <Dialog
      title={t("upd.serversTitle")}
      onClose={running ? () => {} : onClose}
      footer={
        <>
          <button className="btn btn-ghost btn-sm" type="button" disabled={running} onClick={onClose}>
            {op?.state === "done" ? t("wz.close") : t("cancel")}
          </button>
          {!id && (
            <button className="btn btn-primary btn-sm" type="button" disabled={status.outdated.length === 0 && !restart} onClick={() => void go()}>
              {t("upd.serversGo")}
            </button>
          )}
          {op?.state === "failed" && (
            <button className="btn btn-primary btn-sm" type="button" onClick={() => setId(null)}>
              {t("wz.retry")}
            </button>
          )}
        </>
      }
    >
      {!id && (
        <>
          <p className="muted small">{t("upd.serversText", { v: status.current })}</p>
          {status.outdated.length === 0 ? (
            <p className="muted small">{t("upd.serversNone")}</p>
          ) : (
            <ul className="addr-list">
              {status.outdated.map((s) => (
                <li key={s.id}>
                  <b>{s.name}</b> <span className="muted mono">{s.version}</span>
                </li>
              ))}
            </ul>
          )}
          <label className="check">
            <input type="checkbox" checked={restart} onChange={(e) => setRestart(e.target.checked)} /> {t("upd.restartTunnels")}
          </label>
        </>
      )}
      {error && <p className="err small">{error}</p>}
      <Checklist op={op} />
      {op?.state === "done" && <p className="ok small">{t("upd.serversDone")}</p>}
      {op?.state === "failed" && <p className="err small">{t("tun.failed", { why: describeError(t, op.error ?? "", names) })}</p>}
    </Dialog>
  );
}

/** Reads the panel's version until it is not `from` any more (or a minute and a half pass). */
async function waitForNewVersion(from: string): Promise<string | null> {
  for (let i = 0; i < 45; i++) {
    await new Promise((r) => setTimeout(r, 2000));
    try {
      const r = await fetch("./api/version", { credentials: "same-origin" });
      const v = ((await r.json()) as { version?: string }).version;
      if (v && v !== from) return v;
    } catch {
      // the panel is restarting
    }
  }
  return null;
}

export function UpdateDialog({ status, onClose }: { status: UpdateStatus; onClose: () => void }) {
  const { t } = useApp();
  const latest = status.latest;
  const [confirm, setConfirm] = useState(false);
  const [id, setId] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [phase, setPhase] = useState<"ready" | "running" | "restarting" | "done" | "rolled" | "lost">("ready");
  const op = useOp(id);
  const started = useRef(false);

  // The operation ends when the new program is in place and the panel is about to restart;
  // then the browser waits for it, and reads what the swap said.
  useEffect(() => {
    if (!op || started.current) return;
    if (op.state === "failed") {
      setPhase("ready");
      return;
    }
    if (op.state === "done") {
      started.current = true;
      setPhase("restarting");
      void waitForNewVersion(status.current).then(async (v) => {
        if (v) {
          setPhase("done");
          setTimeout(() => location.reload(), 1500);
          return;
        }
        // The version did not change: the swap may have been put back.
        try {
          const s = await api.update();
          setPhase(s.last_result && !s.last_result.ok ? "rolled" : "lost");
        } catch {
          setPhase("lost");
        }
      });
    }
  }, [op, status.current]);

  const go = async () => {
    setError("");
    try {
      setId((await api.applyUpdate(confirm)).op);
      setPhase("running");
    } catch (e) {
      setError(e instanceof ApiError ? describeError(t, e.code) : String(e));
    }
  };

  if (!latest) return null;
  const busy = phase === "running" || phase === "restarting";
  return (
    <Dialog
      title={t("upd.title", { v: latest.version })}
      onClose={busy ? () => {} : onClose}
      footer={
        <>
          <button className="btn btn-ghost btn-sm" type="button" disabled={busy} onClick={onClose}>
            {phase === "done" ? t("wz.close") : t("cancel")}
          </button>
          {phase === "ready" && (
            <button className="btn btn-primary btn-sm" type="button" disabled={status.major && !confirm} onClick={() => void go()}>
              {t("upd.apply")}
            </button>
          )}
        </>
      }
    >
      {phase === "ready" && (
        <>
          <p className="muted small">{t("upd.text", { from: status.current, to: latest.version })}</p>
          {latest.notes && <pre className="code notes">{latest.notes}</pre>}
          {status.major && (
            <label className="check">
              <input type="checkbox" checked={confirm} onChange={(e) => setConfirm(e.target.checked)} /> {t("upd.major")}
            </label>
          )}
          <p className="muted small">{t("upd.servers")}</p>
        </>
      )}
      {error && <p className="err small">{error}</p>}
      <Checklist op={op} />
      {op?.state === "failed" && <p className="err small">{t("tun.failed", { why: describeError(t, op.error ?? "") })}</p>}
      {phase === "restarting" && <p className="muted small">{t("upd.restarting")}</p>}
      {phase === "done" && <p className="ok small">{t("upd.done")}</p>}
      {phase === "rolled" && <p className="err small">{t("upd.rolled")}</p>}
      {phase === "lost" && <p className="err small">{t("upd.lost")}</p>}
    </Dialog>
  );
}

/** Settings > Updates. */
export function UpdatesSection({ status, reload }: { status: UpdateStatus | null; reload: () => void }) {
  const { t, toast } = useApp();
  const [checking, setChecking] = useState(false);
  const [dialog, setDialog] = useState(false);
  const [servers, setServers] = useState(false);
  const row = (title: string, text: string, control: React.ReactNode) => (
    <div className="set-row">
      <div>
        <h3>{title}</h3>
        {text && <p>{text}</p>}
      </div>
      <div className="ctl">{control}</div>
    </div>
  );
  if (!status) return null;
  const check = async () => {
    setChecking(true);
    try {
      await api.checkUpdate();
      reload();
    } catch (e) {
      toast(e instanceof ApiError ? describeError(t, e.code) : String(e));
    } finally {
      setChecking(false);
    }
  };
  const change = async (body: { channel?: "stable" | "beta"; auto?: boolean }) => {
    try {
      await api.updateSettings(body);
      reload();
    } catch {
      toast(t("bak.failed"));
    }
  };
  const state =
    status.error
      ? t("upd.checkFailed", { why: describeError(t, status.error.split(":")[0]) })
      : status.state === "newer" && status.latest
        ? t("upd.isOut", { v: status.latest.version })
        : status.state === "same"
          ? t("upd.upToDate")
          : status.state === "older"
            ? t("upd.older")
            : t("upd.notChecked");
  return (
    <section className="stratum s2">
      <div className="stratum-head">
        <h2>{t("set.updates")}</h2>
      </div>
      {row(
        t("upd.installed", { v: status.current }),
        state,
        <span style={{ display: "flex", gap: "var(--sp-3)", flexWrap: "wrap" }}>
          <button className="btn btn-ghost btn-sm" type="button" disabled={checking || !status.configured} onClick={() => void check()}>
            {t("upd.check")}
          </button>
          {status.state === "newer" && (
            <button className="btn btn-primary btn-sm" type="button" onClick={() => setDialog(true)}>
              {t("upd.apply")}
            </button>
          )}
        </span>,
      )}
      {row(
        t("upd.servers2"),
        status.outdated.length > 0 ? t("upd.behind", { n: status.outdated.length, names: status.outdated.map((x) => x.name).join(", ") }) : t("upd.allCurrent"),
        <button className="btn btn-ghost btn-sm" type="button" onClick={() => setServers(true)}>
          {t("upd.serversGo")}
        </button>,
      )}
      {row(
        t("upd.channel"),
        t("upd.channelText"),
        <Seg
          value={status.channel}
          options={[
            ["stable", t("upd.stable")],
            ["beta", t("upd.beta")],
          ]}
          onChange={(v) => void change({ channel: v })}
        />,
      )}
      {row(
        t("upd.auto"),
        t("upd.autoText"),
        <Seg
          value={status.auto ? "on" : "off"}
          options={[
            ["on", t("upd.on")],
            ["off", t("upd.off")],
          ]}
          onChange={(v) => void change({ auto: v === "on" })}
        />,
      )}
      {status.last_result && !status.last_result.ok &&
        row(t("upd.lastFailed", { v: status.last_result.version }), status.last_result.rolled_back ? t("upd.rolled") : "", <span className="err small">{status.last_result.error}</span>)}
      {dialog && <UpdateDialog status={status} onClose={() => setDialog(false)} />}
      {servers && <ServersDialog status={status} onClose={() => { setServers(false); reload(); }} />}
    </section>
  );
}
