import { useState } from "react";
import { ApiError, api } from "./api";
import { useApp } from "./store";
import { Dialog } from "./ui";

/** The speed test of one tunnel, run by its entry server. */
export function SpeedDialog({ name, onClose }: { name: string; onClose: () => void }) {
  const { t } = useApp();
  const [seconds, setSeconds] = useState(10);
  const [udp, setUdp] = useState(true);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string; error?: string | null } | null>(null);
  const [error, setError] = useState("");

  const run = async () => {
    setBusy(true);
    setError("");
    setResult(null);
    try {
      setResult(await api.speedtest(name, seconds, 4, udp));
    } catch (e) {
      setError(e instanceof ApiError ? e.code : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title={t("speed.title", { name })}
      onClose={busy ? () => {} : onClose}
      footer={
        <>
          <button className="btn btn-ghost btn-sm" type="button" disabled={busy} onClick={onClose}>
            {t("wz.close")}
          </button>
          <button className="btn btn-primary btn-sm" type="button" disabled={busy} onClick={() => void run()}>
            {t(result ? "wz.retry" : "speed.run")}
          </button>
        </>
      }
    >
      <p className="muted small">{t("speed.text")}</p>
      <div className="field">
        <label htmlFor="sp-sec">{t("speed.seconds")}</label>
        <input className="text mono" id="sp-sec" type="number" min={1} max={60} value={seconds} disabled={busy} onChange={(e) => setSeconds(Math.max(1, Math.min(60, +e.target.value || 10)))} />
      </div>
      <label className="check">
        <input type="checkbox" checked={udp} disabled={busy} onChange={(e) => setUdp(e.target.checked)} /> {t("speed.udp")}
      </label>
      {busy && (
        <div className="waiting">
          <svg className="qloader" viewBox="0 0 180 90" aria-hidden="true">
            <path className="bed" d="M30 58 L158 76" />
            <path className="flow" pathLength={1} d="M30 58 L158 76" />
            <path className="shaft" d="M38 26 V58 M84 26 V64 M130 26 V70" />
            <path className="ground" d="M8 26 H172" />
            <path className="mound" d="M30 26 a8 6 0 0 1 16 0Z M76 26 a8 6 0 0 1 16 0Z M122 26 a8 6 0 0 1 16 0Z" />
            <circle className="drop" cx="38" cy="26" r="4" />
            <path className="out" d="M164 68 L178 77 L164 86 Z" />
          </svg>
          <span>{t("speed.running")}</span>
        </div>
      )}
      {error && <p className="err small">{error}</p>}
      {result && (
        <>
          {!result.ok && <p className="err small">{result.error ?? t("speed.failed")}</p>}
          {result.text && (
            <pre className="code" dir="ltr">
              {result.text}
            </pre>
          )}
        </>
      )}
    </Dialog>
  );
}

const fileName = () => `kariz-backup-${new Date().toISOString().slice(0, 10)}.kzb`;

/** Downloads the panel's backup, locked with a passphrase. */
export function BackupDialog({ onClose }: { onClose: () => void }) {
  const { t, toast } = useApp();
  const [pass, setPass] = useState("");
  const [again, setAgain] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  const save = async () => {
    setError("");
    if (pass.length < 10) return setError(t("bak.short"));
    if (pass !== again) return setError(t("bak.mismatch"));
    setBusy(true);
    try {
      const { data } = await api.backup(pass);
      const bytes = Uint8Array.from(atob(data), (c) => c.charCodeAt(0));
      const url = URL.createObjectURL(new Blob([bytes], { type: "application/octet-stream" }));
      const a = document.createElement("a");
      a.href = url;
      a.download = fileName();
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 2000);
      toast(t("bak.saved"));
      onClose();
    } catch (e) {
      setError(e instanceof ApiError && e.code === "short_passphrase" ? t("bak.short") : t("bak.failed"));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title={t("bak.title")}
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-ghost btn-sm" type="button" onClick={onClose}>
            {t("cancel")}
          </button>
          <button className="btn btn-primary btn-sm" type="button" disabled={busy} onClick={() => void save()}>
            {t("bak.download")}
          </button>
        </>
      }
    >
      <p className="muted small">{t("bak.text")}</p>
      <div className="field">
        <label htmlFor="bk-1">{t("bak.pass")}</label>
        <input className="text" id="bk-1" type="password" autoComplete="new-password" value={pass} onChange={(e) => setPass(e.target.value)} />
      </div>
      <div className="field">
        <label htmlFor="bk-2">{t("bak.again")}</label>
        <input className="text" id="bk-2" type="password" autoComplete="new-password" value={again} onChange={(e) => setAgain(e.target.value)} />
        <span className="err">{error}</span>
      </div>
    </Dialog>
  );
}

/** Reads a backup file back into the panel. */
export function RestoreDialog({ onClose, onDone }: { onClose: () => void; onDone: () => void }) {
  const { t, num } = useApp();
  const [file, setFile] = useState<File | null>(null);
  const [pass, setPass] = useState("");
  const [replace, setReplace] = useState(false);
  const [error, setError] = useState("");
  const [count, setCount] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);

  const go = async () => {
    if (!file) return setError(t("rst.pick"));
    setBusy(true);
    setError("");
    try {
      const buf = new Uint8Array(await file.arrayBuffer());
      let bin = "";
      for (let i = 0; i < buf.length; i += 0x8000) bin += String.fromCharCode(...buf.subarray(i, i + 0x8000));
      const r = await api.restore(pass, btoa(bin), replace);
      setCount(r.servers);
      onDone();
    } catch (e) {
      const code = e instanceof ApiError ? e.code : "";
      setError(
        code === "not_empty" ? t("rst.notEmpty") : code === "wrong_passphrase" ? t("rst.wrong") : code === "not_a_backup" || code === "bad_backup" ? t("rst.notBackup") : t("bak.failed"),
      );
      if (code === "not_empty") setReplace(true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title={t("rst.title")}
      onClose={onClose}
      footer={
        count === null ? (
          <>
            <button className="btn btn-ghost btn-sm" type="button" onClick={onClose}>
              {t("cancel")}
            </button>
            <button className="btn btn-primary btn-sm" type="button" disabled={busy} onClick={() => void go()}>
              {t("rst.go")}
            </button>
          </>
        ) : (
          <button className="btn btn-primary btn-sm" type="button" onClick={onClose}>
            {t("wz.close")}
          </button>
        )
      }
    >
      {count === null ? (
        <>
          <p className="muted small">{t("rst.text")}</p>
          <div className="field">
            <label htmlFor="rs-file">{t("rst.file")}</label>
            <input className="text" id="rs-file" type="file" accept=".kzb" onChange={(e) => setFile(e.target.files?.[0] ?? null)} />
          </div>
          <div className="field">
            <label htmlFor="rs-pass">{t("bak.pass")}</label>
            <input className="text" id="rs-pass" type="password" autoComplete="off" value={pass} onChange={(e) => setPass(e.target.value)} />
          </div>
          {replace && (
            <label className="check">
              <input type="checkbox" checked={replace} onChange={(e) => setReplace(e.target.checked)} /> {t("rst.replace")}
            </label>
          )}
          <span className="err small">{error}</span>
        </>
      ) : (
        <>
          <p className="ok">{t("rst.done", { n: num(count) })}</p>
          <p className="muted small">{t("rst.restart")}</p>
        </>
      )}
    </Dialog>
  );
}
