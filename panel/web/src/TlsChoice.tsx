import { useState } from "react";
import { ApiError, api } from "./api";
import { useApp } from "./store";
import { Icon, Seg } from "./ui";

/** The certificate a wss tunnel's listening side serves: a self-signed one (the other side pins it), a real one from Let's Encrypt, or one of your own whose files are on the server. */
export interface TlsState {
  /** `self`: made on the server, pinned. `real`: Let's Encrypt, for a domain or an IPv4 address. `own`: files of your own on the server. */
  mode: "self" | "real" | "own";
  /** The domain or address the certificate is for (what the other side dials). */
  host: string;
  email: string;
  /** The files, once the certificate is there. */
  cert: string;
  key: string;
}

export const emptyTls = (): TlsState => ({ mode: "self", host: "", email: "", cert: "", key: "" });

/** A domain name or an IPv4 address: what a certificate can be issued for. */
const hostLooksRight = (host: string) => /^(\d{1,3}\.){3}\d{1,3}$/.test(host) || /^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}$/.test(host);

/** An absolute path of a PEM file on the server (the agent takes no other kind). */
export const certPathLooksRight = (p: string) =>
  p.startsWith("/") && /\.(pem|crt|cer|key)$/.test(p) && !p.includes("..") && !p.includes("//") && /^[A-Za-z0-9/._-]+$/.test(p) && !/^\/(proc|sys|dev)\//.test(p);

/** A certificate path the manager's Let's Encrypt gave (anything else is one of your own). */
export const isLetsEncrypt = (p: string) => p.startsWith("/etc/letsencrypt/live/");

/** The certificate is all there: nothing to pin, a real one with its files, or your own files and the name they are for. */
export const tlsReady = (s: TlsState) =>
  s.mode === "self" ||
  (s.mode === "real" && !!s.cert && !!s.key) ||
  (s.mode === "own" && certPathLooksRight(s.cert) && certPathLooksRight(s.key) && hostLooksRight(s.host.toLowerCase()));

/** The certificate options of a wss tunnel, with a button that gets the real one on the listening server. */
export function TlsChoice({
  value,
  onChange,
  server,
  serverName,
  onHost,
}: {
  value: TlsState;
  onChange: (v: TlsState) => void;
  /** The server that listens (the certificate is made there). */
  server: string;
  serverName: string;
  /** The certificate is for this host: the other side should dial it. */
  onHost: (host: string) => void;
}) {
  const { t } = useApp();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const host = value.host.trim().toLowerCase();
  const looksRight = hostLooksRight(host);

  const get = async () => {
    setError("");
    setBusy(true);
    try {
      const r = await api.tunnelCert(server, host, value.email.trim() || undefined);
      if (!r.ok || !r.cert || !r.key) {
        setError(certError(t, r.error));
        return;
      }
      onChange({ ...value, host, cert: r.cert, key: r.key });
      onHost(host);
    } catch (e) {
      setError(e instanceof ApiError ? certError(t, e.code) : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="field tls-choice">
      <span className="label">{t("tls.title")}</span>
      <Seg
        value={value.mode}
        options={[
          ["self", t("tls.self")],
          ["real", t("tls.real")],
          ["own", t("tls.own")],
        ]}
        // Let's Encrypt's files are not your own: switching between the two starts them empty.
        onChange={(mode) => onChange({ ...value, mode, ...((mode === "own") !== (value.mode === "own") ? { cert: "", key: "" } : {}) })}
      />
      <span className="help">{t(`tls.${value.mode}.d`)}</span>
      {value.mode === "own" && (
        <div className="tls-real">
          <div className="grid-2">
            <div className="field">
              <label htmlFor="tls-cert">{t("tls.certFile")}</label>
              <input
                className="text mono"
                id="tls-cert"
                dir="ltr"
                value={value.cert}
                placeholder="/etc/ssl/kariz/fullchain.pem"
                aria-invalid={!!value.cert && !certPathLooksRight(value.cert)}
                onChange={(e) => onChange({ ...value, cert: e.target.value.trim() })}
              />
              <span className="help">{t("tls.certFile.d", { server: serverName })}</span>
            </div>
            <div className="field">
              <label htmlFor="tls-key">{t("tls.keyFile")}</label>
              <input
                className="text mono"
                id="tls-key"
                dir="ltr"
                value={value.key}
                placeholder="/etc/ssl/kariz/privkey.pem"
                aria-invalid={!!value.key && !certPathLooksRight(value.key)}
                onChange={(e) => onChange({ ...value, key: e.target.value.trim() })}
              />
              <span className="help">{t("tls.keyFile.d")}</span>
            </div>
            <div className="field">
              <label htmlFor="tls-own-host">{t("tls.host")}</label>
              <input
                className="text mono"
                id="tls-own-host"
                dir="ltr"
                value={value.host}
                placeholder="tunnel.example.com"
                aria-invalid={!!host && !looksRight}
                onChange={(e) => onChange({ ...value, host: e.target.value.trim() })}
                onBlur={() => looksRight && onHost(host)}
              />
              <span className="help">{t("tls.own.host.d", { server: serverName })}</span>
            </div>
          </div>
          {((value.cert && !certPathLooksRight(value.cert)) || (value.key && !certPathLooksRight(value.key))) && (
            <p className="err small" role="alert">
              {t("tls.err.path")}
            </p>
          )}
          <p className="muted small">{t("tls.own.renew")}</p>
        </div>
      )}
      {value.mode === "real" && (
        <div className="tls-real">
          <div className="grid-2">
            <div className="field">
              <label htmlFor="tls-host">{t("tls.host")}</label>
              <input
                className="text mono"
                id="tls-host"
                dir="ltr"
                value={value.host}
                placeholder="tunnel.example.com"
                disabled={busy}
                onChange={(e) => onChange({ ...value, host: e.target.value.trim(), cert: "", key: "" })}
              />
              <span className="help">{t("tls.host.d", { server: serverName })}</span>
            </div>
            <div className="field">
              <label htmlFor="tls-email">{t("tls.email")}</label>
              <input className="text mono" id="tls-email" dir="ltr" type="email" value={value.email} disabled={busy} onChange={(e) => onChange({ ...value, email: e.target.value.trim() })} />
              <span className="help">{t("tls.email.d")}</span>
            </div>
          </div>
          <div className="tls-act">
            <button className="btn btn-primary btn-sm" type="button" disabled={busy || !looksRight} onClick={() => void get()}>
              {busy ? t("tls.getting") : value.cert ? t("tls.again") : t("tls.get")}
            </button>
            {value.cert && !busy && (
              <span className="ok small tls-ok">
                <Icon name="check" size={14} /> {t("tls.have", { host })}
              </span>
            )}
          </div>
          {busy && <p className="muted small">{t("tls.wait")}</p>}
          {!busy && !value.cert && <p className="muted small">{t("tls.port80", { server: serverName })}</p>}
          {error && (
            <p className="err small" role="alert">
              {error}
            </p>
          )}
        </div>
      )}
    </div>
  );
}

function certError(t: (k: string, v?: Record<string, string | number>) => string, code: string | null): string {
  if (!code) return t("tls.failed", { why: "?" });
  const known = ["bad_host", "bad_email", "no_manager"];
  return known.includes(code) ? t(`tls.err.${code}`) : t("tls.failed", { why: code });
}
