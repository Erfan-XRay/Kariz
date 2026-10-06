import { useEffect, useRef, useState } from "react";
import { ApiError, api } from "./api";
import type { Bench, BenchCandidate, ServerInfo } from "./api";
import { useApp } from "./store";
import { Icon, useAgo } from "./ui";

/** What using a benchmark result sets: the core transport, the direction, and the network of a GRE path. */
export interface BenchChoice {
  transport: string;
  mode: "reverse" | "direct";
  network: string | null;
}

export const choiceOf = (c: BenchCandidate): BenchChoice => ({ transport: c.transport, mode: c.mode, network: c.path === "gre" ? c.network : null });

const LABELS: Record<string, string> = { tcpmux: "TCP", kcp: "KCP", ws: "WS", wss: "WSS", quic: "QUIC" };
/** A transport as the benchmark shows it. */
export const benchLabel = (transport: string) => LABELS[transport] ?? transport.toUpperCase();

/** The candidates a run measures for speed (the best few of the probe stage). */
const SPEED_TOP = 3;
/** About how long each stage takes, in seconds, for the time-left estimate. */
const PROBE_SECONDS = 8;
const SPEED_SECONDS = 4.5;

type T = (k: string, v?: Record<string, string | number>) => string;

/** A failure in words: most are "no answer", which is what a filter or a firewall looks like. */
function reason(t: T, error: string | null, names: Map<string, string>): string {
  const e = (error ?? "").toLowerCase();
  if (e === "agent_old") return t("bench.err.old");
  if (e.startsWith("no_address:")) return t("bench.err.noAddr", { name: names.get(e.split(":")[1]) ?? e.split(":")[1] });
  if (e === "busy") return t("bench.err.agentBusy");
  if (e.includes("refused")) return t("bench.err.refused");
  if (e.includes("reset") || e.includes("broken pipe") || e.includes("closed")) return t("bench.err.reset");
  if (e.includes("nothing came back")) return t("bench.err.silent");
  if (e.includes("timed out") || e.includes("in time") || e.includes("nobody came") || e.includes("timeout")) return t("bench.err.timeout");
  if (e.includes("not connected")) return t("bench.err.offline");
  return error ?? t("bench.err.unknown");
}

const speedOf = (c: BenchCandidate) => (c.download_mbps != null && c.upload_mbps != null ? 0.7 * c.download_mbps + 0.3 * c.upload_mbps : null);
const tone = (score: number) => (score >= 80 ? "good" : score >= 60 ? "mid" : "low");

function ScoreRing({ score, size = 76 }: { score: number; size?: number }) {
  const { t, num } = useApp();
  const r = 30;
  const c = 2 * Math.PI * r;
  return (
    <div className={`score-ring ${tone(score)}`} style={{ width: size, height: size }} role="img" aria-label={t("bench.scoreOf", { n: num(score) })}>
      <svg viewBox="0 0 72 72" aria-hidden="true">
        <circle cx="36" cy="36" r={r} className="track" />
        <circle cx="36" cy="36" r={r} className="fill" strokeDasharray={`${(score / 100) * c} ${c}`} />
      </svg>
      <b className="num">{num(score)}</b>
    </div>
  );
}

/**
 * The benchmark between two servers: which transport, in which direction, gets through best. Shows the last
 * result of the pair if there is one, runs a new one (about 20 seconds, followed live), and offers each result
 * with `onApply`. `isCurrent` marks the one the tunnel (or the wizard) has now.
 */
export function BenchPanel({
  entry,
  exit,
  profile,
  port,
  servers,
  applyLabel,
  currentLabel,
  isCurrent,
  context,
  onApply,
}: {
  entry: string;
  exit: string;
  profile: string;
  /** The port the tunnel would listen on: tried first, so a filter on it shows. 0: any. */
  port: number;
  servers: ServerInfo[];
  applyLabel: string;
  currentLabel: string;
  isCurrent?: (c: BenchCandidate) => boolean;
  /** Where it is: the wizard (the current one is what is picked there) or a tunnel's page (what it runs on). */
  context: "wizard" | "tunnel";
  onApply: (choice: BenchChoice, c: BenchCandidate) => void;
}) {
  const { t, num, digitsOf } = useApp();
  const ago = useAgo();
  const [bench, setBench] = useState<Bench | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState("");
  const [now, setNow] = useState(Date.now());
  const [showAll, setShowAll] = useState(false);
  const names = new Map(servers.map((s) => [s.id, s.name]));
  const name = (id: string) => names.get(id) ?? id;
  const running = !!bench && (bench.state === "probe" || bench.state === "speed");
  const runId = useRef<string | null>(null);

  // The last result of this pair, if there is one.
  useEffect(() => {
    let alive = true;
    setLoaded(false);
    setBench(null);
    api
      .benchLast(entry, exit)
      .then((r) => alive && setBench(r.bench))
      .catch(() => {})
      .finally(() => alive && setLoaded(true));
    return () => {
      alive = false;
    };
  }, [entry, exit]);

  // A run is followed until it ends.
  useEffect(() => {
    if (!running || !bench) return;
    const id = bench.id;
    runId.current = id;
    const poll = setInterval(() => {
      void api
        .benchState(id)
        .then((b) => runId.current === id && setBench(b))
        .catch(() => {});
    }, 400);
    const clock = setInterval(() => setNow(Date.now()), 250);
    return () => {
      clearInterval(poll);
      clearInterval(clock);
    };
  }, [running, bench?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  const start = async () => {
    setError("");
    setStarting(true);
    setShowAll(false);
    try {
      const { id } = await api.benchStart(entry, exit, profile, port);
      setNow(Date.now());
      setBench(await api.benchState(id));
    } catch (e) {
      const code = e instanceof ApiError ? e.code : String(e);
      if (code.startsWith("offline:")) setError(t("bench.err.serverOff", { name: name(code.split(":")[1]) }));
      else if (code === "busy") setError(t("bench.err.running"));
      else setError(t("bench.err.start", { why: code }));
    } finally {
      setStarting(false);
    }
  };

  const stop = () => {
    if (bench) void api.benchStop(bench.id).catch(() => {});
  };

  const startButton = (label: string, primary: boolean) => (
    <button className={`btn btn-sm ${primary ? "btn-primary" : "btn-ghost"}`} type="button" disabled={starting} onClick={() => void start()}>
      {primary && <span className="shine" />}
      <Icon name={primary ? "gauge" : "refresh"} size={18} />
      {starting ? t("bench.starting") : label}
    </button>
  );

  if (!loaded) return <div className="bench-skel" aria-busy="true" />;

  // ---- nothing yet ----
  if (!bench) {
    return (
      <div className="bench">
        <div className="bench-intro">
          <div className="bench-intro-icon" aria-hidden="true">
            <Icon name="gauge" size={26} />
          </div>
          <div className="bench-intro-text">
            <h4>{t("bench.introTitle")}</h4>
            <p>{t("bench.introText", { a: `⁨${name(entry)}⁩`, b: `⁨${name(exit)}⁩` })}</p>
            <ul className="bench-facts">
              <li>
                <Icon name="clock" size={14} />
                {t("bench.fact.time")}
              </li>
              <li>
                <Icon name="refresh" size={14} />
                {t("bench.fact.both")}
              </li>
              <li>
                <Icon name="shield" size={14} />
                {t("bench.fact.safe")}
              </li>
            </ul>
          </div>
          {startButton(t("bench.run"), true)}
        </div>
        {error && (
          <p className="err small" role="alert">
            {error}
          </p>
        )}
      </div>
    );
  }

  const cands = bench.candidates;
  const modes = (["reverse", "direct"] as const).filter((m) => cands.some((c) => c.mode === m));
  const listenerOf = (m: "reverse" | "direct") => (m === "reverse" ? bench.entry : bench.exit);

  // ---- running ----
  if (running) {
    const elapsed = Math.max(0, (now - bench.started) / 1000);
    const probed = cands.filter((c) => c.state !== "wait" && c.state !== "probe").length;
    const measured = cands.filter((c) => c.download_mbps != null).length;
    const measuring = cands.some((c) => c.state === "speed");
    const fraction =
      bench.state === "probe"
        ? 0.35 * (probed / Math.max(1, cands.length))
        : 0.35 + 0.65 * Math.min(1, (measured + (measuring ? 0.5 : 0)) / SPEED_TOP);
    const left =
      bench.state === "probe"
        ? Math.max(1, PROBE_SECONDS - elapsed) + SPEED_TOP * SPEED_SECONDS
        : Math.max(1, (SPEED_TOP - measured - (measuring ? 0.5 : 0)) * SPEED_SECONDS);
    return (
      <div className="bench is-running">
        <div className="bench-run-head">
          <ol className="bench-stages" aria-label={t("bench.stages")}>
            {(["probe", "speed"] as const).map((s, i) => (
              <li key={s} className={bench.state === s ? "now" : bench.state === "speed" && s === "probe" ? "done" : ""} aria-current={bench.state === s ? "step" : undefined}>
                <span className="dot">{bench.state === "speed" && s === "probe" ? <Icon name="check" size={12} /> : num(i + 1)}</span>
                <span>
                  <b>{t(`bench.stage.${s}`)}</b>
                  <small>{t(`bench.stage.${s}.d`)}</small>
                </span>
              </li>
            ))}
          </ol>
          <span className="grow" />
          <span className="bench-left num" aria-hidden="true">
            {t("bench.left", { n: num(Math.ceil(left)) })}
          </span>
          <button className="btn btn-ghost btn-sm" type="button" onClick={stop}>
            <Icon name="pause" size={16} />
            {t("bench.stop")}
          </button>
        </div>
        <div className="bench-bar" role="progressbar" aria-label={t("bench.progress")} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(fraction * 100)}>
          <i style={{ width: `${Math.max(4, fraction * 100)}%` }} />
        </div>
        <p className="sr-only" role="status">
          {t(`bench.stage.${bench.state}`)}
        </p>
        <div className={`bench-board cols-${modes.length}`}>
          {modes.map((m) => (
            <div key={m} className="bench-lane">
              <h5>
                {t(`bench.mode.${m}`)}
                <small>{t("bench.listens", { name: `⁨${name(listenerOf(m))}⁩` })}</small>
              </h5>
              <ul>
                {cands
                  .filter((c) => c.mode === m)
                  .map((c) => (
                    <li key={c.key} className={`bench-row st-${c.state}`}>
                      <span className="bench-glyph" aria-hidden="true">
                        {c.state === "ok" && <Icon name="check" size={14} />}
                        {c.state === "fail" && <Icon name="x" size={14} />}
                        {c.state === "speed" && (
                          <span className="bars">
                            <i />
                            <i />
                            <i />
                          </span>
                        )}
                      </span>
                      <span className="bench-t mono" dir="ltr">
                        {benchLabel(c.transport)}
                      </span>
                      {c.path === "gre" && <span className="tag via">GRE</span>}
                      <span className="grow" />
                      <span className="bench-v small">
                        {c.state === "wait" && t("bench.st.wait")}
                        {c.state === "probe" && t("bench.st.probe")}
                        {c.state === "speed" && t("bench.st.speed")}
                        {c.state === "ok" &&
                          (c.download_mbps != null ? (
                            <span className="num">
                              ↓ {num(c.download_mbps, c.download_mbps < 10 ? 1 : 0)} <bdi dir="ltr">Mbps</bdi>
                            </span>
                          ) : c.latency ? (
                            <span className="num">
                              {num(c.latency.p50_ms, c.latency.p50_ms < 10 ? 1 : 0)} <bdi dir="ltr">ms</bdi>
                            </span>
                          ) : null)}
                        {c.state === "fail" && <span title={c.error ?? undefined}>{reason(t, c.error, names)}</span>}
                      </span>
                      <span className="sr-only">{t(`bench.st.${c.state}`)}</span>
                    </li>
                  ))}
              </ul>
            </div>
          ))}
        </div>
      </div>
    );
  }

  // ---- results ----
  const ok = cands.filter((c) => c.state === "ok");
  const ranked = cands.filter((c) => c.score != null).sort((a, b) => (b.score ?? 0) - (a.score ?? 0));
  const best = ranked[0];
  const unranked = ok.filter((c) => c.score == null);
  const failed = cands.filter((c) => c.state === "fail");
  const skipped = cands.filter((c) => c.state === "skip");
  const finishedAgo = bench.finished ? ago(Math.max(0, (Date.now() - bench.finished) / 1000)) : null;
  const tookSecs = bench.finished ? Math.round((bench.finished - bench.started) / 1000) : null;
  const testedPort = cands.find((c) => c.port != null)?.port ?? null;
  const old = bench.finished != null && Date.now() - bench.finished > 24 * 3600 * 1000;
  const current = isCurrent ? ranked.find(isCurrent) : undefined;
  const isCurrentTransport = isCurrent ? cands.find(isCurrent)?.transport : undefined;

  // Why the best is the best, in a few words.
  const whys: string[] = [];
  if (best) {
    const fastest = Math.max(...ranked.map((c) => speedOf(c) ?? 0));
    const quickest = Math.min(...ok.map((c) => c.latency?.p50_ms ?? Infinity));
    const share = fastest > 0 ? (speedOf(best) ?? 0) / fastest : 0;
    if (share >= 1) whys.push(t("bench.why.fastest"));
    else if (share >= 0.75) whys.push(t("bench.why.nearFastest", { pct: num(Math.round(share * 100)) }));
    if ((best.latency?.p50_ms ?? Infinity) <= quickest) whys.push(t("bench.why.quickest"));
    if ((best.parts?.stability ?? 0) >= 95) whys.push(t("bench.why.steady"));
    const runner = ranked.find((c) => c.transport !== best.transport && speedOf(c));
    const ratio = runner ? (speedOf(best) ?? 0) / (speedOf(runner) ?? 1) : 0;
    if (runner && ratio >= 1.15) whys.push(t("bench.why.times", { x: num(ratio, 1), other: benchLabel(runner.transport) }));
    if (!whys.length) whys.push(t("bench.why.balance"));
  }

  // A direction where nothing got through says which server takes no connections.
  const blockedMode = modes.length === 2 ? modes.find((m) => ok.length > 0 && !ok.some((c) => c.mode === m)) : undefined;
  const better = best && current && current.key !== best.key && (best.score ?? 0) - (current.score ?? 0) >= 8 ? current : undefined;
  const betterThanNone = best && isCurrent && !current && !ok.some(isCurrent);

  const chips = (c: BenchCandidate) => (
    <>
      <span className="tag">{t(`bench.mode.${c.mode}`)}</span>
      {c.path === "gre" && <span className="tag via">GRE</span>}
      {isCurrent?.(c) && <span className="tag now">{currentLabel}</span>}
    </>
  );

  return (
    <div className="bench">
      {old && (
        <p className="bench-note">
          <Icon name="info" size={16} />
          {t("bench.old")}
        </p>
      )}
      {bench.profile !== profile && (
        <p className="bench-note">
          <Icon name="info" size={16} />
          {t("bench.otherProfile", { p: t(`wz.pf.${bench.profile}`) })}
        </p>
      )}
      {bench.state === "stopped" && (
        <p className="bench-note">
          <Icon name="pause" size={16} />
          {t("bench.stopped")}
        </p>
      )}
      {bench.state === "failed" && (
        <p className="err small" role="alert">
          {t("bench.err.run", { why: bench.error ?? "" })}
        </p>
      )}

      {best ? (
        <section className={`bench-best ${tone(best.score ?? 0)}`} aria-label={t("bench.recommended")}>
          <ScoreRing score={best.score ?? 0} />
          <div className="bench-best-main">
            <span className="bench-kicker">{t("bench.recommended")}</span>
            <h4>
              <span className="mono" dir="ltr">
                {benchLabel(best.transport)}
              </span>
              <span className="bench-chips">{chips(best)}</span>
            </h4>
            <p className="bench-why">{whys.join(t("bench.and"))}</p>
            <dl className="bench-metrics">
              <div>
                <dt>{t("bench.m.down")}</dt>
                <dd className="num">
                  {best.download_mbps != null ? num(best.download_mbps, best.download_mbps < 10 ? 1 : 0) : "—"} <small dir="ltr">Mbps</small>
                </dd>
              </div>
              <div>
                <dt>{t("bench.m.up")}</dt>
                <dd className="num">
                  {best.upload_mbps != null ? num(best.upload_mbps, best.upload_mbps < 10 ? 1 : 0) : "—"} <small dir="ltr">Mbps</small>
                </dd>
              </div>
              <div>
                <dt>{t("bench.m.rtt")}</dt>
                <dd className="num">
                  {best.latency ? num(best.latency.p50_ms, best.latency.p50_ms < 10 ? 1 : 0) : "—"} <small dir="ltr">ms</small>
                </dd>
              </div>
              <div>
                <dt>{t("bench.m.jitter")}</dt>
                <dd className="num">
                  {best.latency ? num(best.latency.jitter_ms, 1) : "—"} <small dir="ltr">ms</small>
                </dd>
              </div>
              <div>
                <dt>{t("bench.m.loss")}</dt>
                <dd className="num">{best.latency ? t("bench.pctOf", { n: num(lossOf(best), 0) }) : "—"}</dd>
              </div>
            </dl>
            {best.parts && (
              <details className="bench-parts">
                <summary>{t("bench.howScored")}</summary>
                <div className="bench-part-bars">
                  {(["speed", "latency", "stability"] as const).map((k) => (
                    <div key={k}>
                      <span>{t(`bench.part.${k}`)}</span>
                      <span className="bar" aria-hidden="true">
                        <i style={{ width: `${best.parts![k]}%` }} />
                      </span>
                      <b className="num">{num(best.parts![k])}</b>
                    </div>
                  ))}
                </div>
                <p className="small muted">{t(`bench.weights.${bench.profile}`)}</p>
              </details>
            )}
          </div>
          <div className="bench-best-act">
            {isCurrent?.(best) ? (
              <span className="bench-is-now">
                <Icon name="check" size={16} />
                {currentLabel}
              </span>
            ) : (
              <button className="btn btn-primary btn-sm" type="button" onClick={() => onApply(choiceOf(best), best)}>
                <span className="shine" />
                <Icon name="check" size={18} />
                {applyLabel}
              </button>
            )}
          </div>
        </section>
      ) : (
        bench.state !== "failed" && (
          <div className="bench-none">
            <Icon name="alert" size={20} />
            <div>
              <h4>{t("bench.none")}</h4>
              <ul>
                <li>{t("bench.none.1", { port: testedPort ? digitsOf(String(testedPort)) : t("bench.thePort") })}</li>
                <li>{t("bench.none.2")}</li>
                <li>{t("bench.none.3")}</li>
              </ul>
            </div>
          </div>
        )
      )}

      {better && (
        <p className="bench-note good">
          <Icon name="bolt" size={16} />
          {t(context === "wizard" ? "bench.betterPick" : "bench.better", { best: benchLabel(best!.transport), by: num((best!.score ?? 0) - (better.score ?? 0)), now: benchLabel(better.transport) })}
        </p>
      )}
      {betterThanNone && (
        <p className="bench-note">
          <Icon name="info" size={16} />
          {t(context === "wizard" ? "bench.pickFailed" : "bench.currentFailed", { now: benchLabel(isCurrentTransport ?? "") })}
        </p>
      )}
      {blockedMode && (
        <p className="bench-note warn">
          <Icon name="alert" size={16} />
          {t("bench.blocked", { mode: t(`bench.mode.${blockedMode}`), name: `⁨${name(listenerOf(blockedMode))}⁩`, other: t(`bench.mode.${blockedMode === "reverse" ? "direct" : "reverse"}`) })}
        </p>
      )}

      {ranked.length > 1 && (
        <ol className="bench-rank" aria-label={t("bench.ranking")}>
          {ranked.slice(1).map((c, i) => (
            <li key={c.key}>
              <span className="bench-pos num">{num(i + 2)}</span>
              <span className="bench-t mono" dir="ltr">
                {benchLabel(c.transport)}
              </span>
              <span className="bench-chips">{chips(c)}</span>
              <span className="bench-score">
                <span className={`bar ${tone(c.score ?? 0)}`} aria-hidden="true">
                  <i style={{ width: `${c.score ?? 0}%` }} />
                </span>
                <b className="num">{num(c.score ?? 0)}</b>
              </span>
              <span className="bench-mini small num">
                <span>
                  ↓ {c.download_mbps != null ? num(c.download_mbps, 0) : "—"} <bdi dir="ltr">Mbps</bdi>
                </span>
                <span>
                  {c.latency ? num(c.latency.p50_ms, 0) : "—"} <bdi dir="ltr">ms</bdi>
                </span>
              </span>
              {isCurrent?.(c) ? (
                <span className="bench-act-space" />
              ) : (
                <button className="btn btn-quiet btn-sm" type="button" onClick={() => onApply(choiceOf(c), c)} aria-label={`${applyLabel}: ${benchLabel(c.transport)} ${t(`bench.mode.${c.mode}`)}`}>
                  {applyLabel}
                </button>
              )}
            </li>
          ))}
        </ol>
      )}

      {unranked.length + failed.length + skipped.length > 0 && (
        <div className="bench-rest">
          <button className="linklike small" type="button" aria-expanded={showAll} onClick={() => setShowAll(!showAll)}>
            <Icon name="chevron" size={14} />
            {t("bench.rest", {
              list: [
                unranked.length && t("bench.rest.ok", { n: num(unranked.length) }),
                failed.length && t("bench.rest.fail", { n: num(failed.length) }),
                skipped.length && t("bench.rest.skip", { n: num(skipped.length) }),
              ]
                .filter(Boolean)
                .join(t("bench.and")),
            })}
          </button>
          {showAll && (
            <ul>
              {unranked.map((c) => (
                <li key={c.key} className="st-ok">
                  <Icon name="check" size={14} />
                  <span className="bench-t mono" dir="ltr">
                    {benchLabel(c.transport)}
                  </span>
                  <span className="bench-chips">{chips(c)}</span>
                  <span className="grow" />
                  <span className="small muted">
                    {t("bench.notMeasured", { ms: c.latency ? num(c.latency.p50_ms, 0) : "—" })}
                  </span>
                </li>
              ))}
              {failed.map((c) => (
                <li key={c.key} className="st-fail">
                  <Icon name="x" size={14} />
                  <span className="bench-t mono" dir="ltr">
                    {benchLabel(c.transport)}
                  </span>
                  <span className="bench-chips">{chips(c)}</span>
                  <span className="grow" />
                  <span className="small" title={c.error ?? undefined}>
                    {reason(t, c.error, names)}
                  </span>
                </li>
              ))}
              {skipped.map((c) => (
                <li key={c.key} className="st-skip">
                  <span className="bench-glyph" aria-hidden="true" />
                  <span className="bench-t mono" dir="ltr">
                    {benchLabel(c.transport)}
                  </span>
                  <span className="bench-chips">{chips(c)}</span>
                  <span className="grow" />
                  <span className="small muted">{t("bench.st.skip")}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      <div className="bench-foot">
        <span className="small muted">
          {[
            finishedAgo && t("bench.when", { ago: finishedAgo }),
            tookSecs != null && t("bench.took", { n: num(tookSecs) }),
            testedPort != null && digitsOf(t("bench.port", { port: String(testedPort) })),
          ]
            .filter(Boolean)
            .join(" — ")}
        </span>
        <span className="grow" />
        {startButton(t("bench.again"), false)}
      </div>
      <p className="small muted bench-snapshot">{t("bench.snapshot")}</p>
      {error && (
        <p className="err small" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

/** Lost round trips of a candidate's probe, in percent. */
function lossOf(c: BenchCandidate): number {
  const l = c.latency;
  if (!l || l.sent === 0) return 0;
  return (100 * Math.max(0, l.sent - l.received)) / l.sent;
}
