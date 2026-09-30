import { useState } from "react";
import { ApiError, api } from "./api";
import { useApp } from "./store";
import { Dialog } from "./ui";

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
