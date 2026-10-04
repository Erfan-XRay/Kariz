import { useEffect, useRef, useState } from "react";
import { api, ApiError } from "./api";
import type { LinkTransport, ServerInfo } from "./api";
import { useApp } from "./store";
import { CodeBlock, Dialog, Icon, Seg, useAgo } from "./ui";

const TRANSPORTS: LinkTransport[] = ["auto", "tcpmux", "kcp", "wss", "quic"];

/**
 * A server whose agent is not connected, or one that is to change how its agent reaches the
 * panel: what the panel knows, what to look at on that server, and a join code that makes the
 * new agent that same server again (same name, same tunnels, same private network links).
 * Also its name.
 */
export function ServerFixDialog({ server, servers, agentsOn, onChanged, onClose }: { server: ServerInfo; servers: ServerInfo[]; agentsOn: boolean; onChanged: () => void; onClose: () => void }) {
  const { t, toast, digitsOf } = useApp();
  const ago = useAgo();
  const live = servers.find((s) => s.id === server.id) ?? server;
  const wasOffline = useRef(!server.online);
  const [name, setName] = useState(server.name);
  const [host, setHost] = useState(location.hostname.replace(/^\[|\]$/g, ""));
  const [transport, setTransport] = useState<LinkTransport>("auto");
  const [own, setOwn] = useState<{ v4: string | null; v6: string | null; agent_port: number | null } | null>(null);
  const [code, setCode] = useState("");
  const [left, setLeft] = useState(0);
  const [error, setError] = useState("");
  const [nameError, setNameError] = useState("");
  const back = !!code && wasOffline.current && live.online;

  useEffect(() => {
    let alive = true;
    api
      .panelAddresses()
      .then((a) => alive && setOwn(a))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);
  useEffect(() => {
    if (left <= 0 || back) return;
    const id = setInterval(() => setLeft((v) => Math.max(0, v - 1)), 1000);
    return () => clearInterval(id);
  }, [left > 0, back]); // eslint-disable-line react-hooks/exhaustive-deps

  const saveName = async () => {
    setNameError("");
    try {
      await api.renameServer(server.id, name.trim());
      toast(t("fix.renamed"), "ok");
      onChanged();
    } catch (e) {
      setNameError(e instanceof ApiError && e.code === "name_taken" ? t("fix.nameTaken") : t("fix.nameBad"));
    }
  };

  const make = async () => {
    setError("");
    try {
      const made = await api.reconnectServer(server.id, host.trim(), transport);
      setCode(made.code);
      setLeft(made.valid_for);
    } catch (e) {
      setError(e instanceof ApiError && e.code === "agents_off" ? t("add.off") : t("add.bad"));
    }
  };

  const mmss = `${String(Math.floor(left / 60)).padStart(2, "0")}:${String(left % 60).padStart(2, "0")}`;
  const port = own?.agent_port ?? null;
  const here = location.hostname.replace(/^\[|\]$/g, "");
  const choices: [string, string][] = [
    ...(own?.v4 ? [[t("add.addr.v4"), own.v4] as [string, string]] : []),
    ...(own?.v6 ? [[t("add.addr.v6"), own.v6] as [string, string]] : []),
    ...(here && here !== own?.v4 && here !== own?.v6 ? [[t("add.addr.here"), here] as [string, string]] : []),
  ];
  const portText = (x: LinkTransport) => (port == null ? "?" : String(x === "wss" ? port + 1 : port));
  const seenAgo = live.last_seen ? ago(Math.max(0, Date.now() / 1000 - live.last_seen)) : null;
  const err = live.last_error;
  const title = live.online ? t("fix.titleOnline", { name: live.name }) : t("fix.title", { name: live.name });

  return (
    <Dialog
      title={title}
      onClose={onClose}
      footer={
        back ? (
          <button
            className="btn btn-primary btn-sm"
            type="button"
            onClick={() => {
              toast(t("fix.connected", { name: live.name }), "ok");
              onChanged();
              onClose();
            }}
          >
            {t("add.done")}
          </button>
        ) : (
          <>
            <button className="btn btn-ghost btn-sm" type="button" onClick={onClose}>
              {code ? t("close") : t("cancel")}
            </button>
            {!code && (
              <button className="btn btn-primary btn-sm" type="button" disabled={!host.trim() || !agentsOn} onClick={() => void make()}>
                {t("fix.make")}
              </button>
            )}
          </>
        )
      }
    >
      {!agentsOn && <p className="err small">{t("add.off")}</p>}

      <div className="fix-state">
        {live.online ? (
          <p>
            <Icon name="check" size={16} /> {t("fix.onlineNow")}
          </p>
        ) : (
          <>
            <p>
              <Icon name="alert" size={16} /> {t("fix.offlineNow")}
              {seenAgo ? ` ${t("fix.lastSeen", { ago: digitsOf(seenAgo) })}` : ` ${t("fix.neverSeen")}`}
            </p>
            {err?.kind === "wrong_key" && <p className="err small">{t("fix.err.wrong_key")}</p>}
            {err?.kind === "link_ended" && (
              <p className="small">
                {t("fix.err.link_ended")} {err.detail && <code dir="ltr">{err.detail}</code>}
              </p>
            )}
            {!err && <p className="small muted">{t("fix.err.none")}</p>}
          </>
        )}
        <span className="help">{t("fix.tunnelsKeep")}</span>
      </div>

      <div className="field">
        <label htmlFor="fix-name">{t("fix.name")}</label>
        <div className="ar-row fix-name-row">
          <input id="fix-name" className="text mono" dir="ltr" value={name} onChange={(e) => setName(e.target.value)} />
          <button className="btn btn-ghost btn-sm" type="button" disabled={!name.trim() || name.trim() === live.name} onClick={() => void saveName()}>
            {t("ar.save")}
          </button>
        </div>
        {nameError && <span className="err">{nameError}</span>}
      </div>

      {!live.online && !code && (
        <div className="field">
          <span className="label">{t("fix.checkTitle")}</span>
          <span className="help">{t("fix.check1")}</span>
          <CodeBlock text="kariz-manager agent status" />
          <span className="help">{t("fix.check2")}</span>
          <CodeBlock text="systemctl restart kariz-agent" />
          <span className="help">{t("fix.check3")}</span>
        </div>
      )}

      {!code && (
        <>
          <div className="field">
            <label htmlFor="fix-host">{t("add.host")}</label>
            {choices.length > 1 && (
              <div className="addr-picks" role="group" aria-label={t("add.host")}>
                {choices.map(([label, value]) => (
                  <button key={value} type="button" className="addr-pick" aria-pressed={host === value} onClick={() => setHost(value)}>
                    <span>{label}</span>
                    <code dir="ltr">{value}</code>
                  </button>
                ))}
              </div>
            )}
            <input className="text mono" id="fix-host" dir="ltr" value={host} onChange={(e) => setHost(e.target.value)} />
            <span className="help">{t("add.addrHelp")}</span>
            <span className="err">{error}</span>
          </div>
          <div className="field">
            <span className="label">{t("fix.protocol")}</span>
            <Seg<LinkTransport> value={transport} options={TRANSPORTS.map((x) => [x, t(`add.x.${x}`)] as [LinkTransport, string])} onChange={setTransport} />
            <span className="help">{t(`add.x.${transport}.d`, { p: portText(transport) })}</span>
            <span className="help">{t("fix.protocolWhy")}</span>
            {transport !== "auto" && <span className="help">{t("add.newAgent")}</span>}
          </div>
          <div className="field">
            <span className="help">{t("fix.codeHelp")}</span>
          </div>
        </>
      )}

      {code && (
        <>
          <div className="field">
            <span className="label">{t("fix.run")}</span>
            <CodeBlock text={`kariz-manager --agent ${code}`} />
            <span className="help">{t("fix.runHelp")}</span>
            <span className="label" style={{ marginTop: "var(--sp-3)" }}>
              {t("fix.runFresh")}
            </span>
            <CodeBlock text={`bash <(curl -fsSL https://raw.githubusercontent.com/Erfan-XRay/Kariz/main/scripts/kariz.sh) --agent ${code}`} />
            {transport !== "auto" && <span className="help">{t("add.via", { t: t(`add.x.${transport}`) })}</span>}
            <span className="countdown">{back ? "" : t("add.valid", { m: digitsOf(mmss) })}</span>
          </div>
          <div className={`waiting ${back ? "done" : ""}`}>
            <span>{back ? t("fix.connected", { name: live.name }) : wasOffline.current ? t("add.waiting") : t("fix.waitingOnline")}</span>
          </div>
        </>
      )}
    </Dialog>
  );
}
