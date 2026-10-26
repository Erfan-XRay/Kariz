import { useApp } from "./store";
import { Icon } from "./ui";
import { Choice } from "./Choice";

export const CIPHERS = ["auto", "aes-256-gcm", "chacha20-poly1305", "none"] as const;
export type Cipher = (typeof CIPHERS)[number];

/** The cipher to send: nothing for `auto`, and nothing for QUIC (always TLS 1.3). */
export const cipherOf = (value: string, transport: string): string | undefined => (value === "auto" || transport === "quic" ? undefined : value);

/** Whether the choice may go on: no encryption has to be confirmed. */
export const cipherReady = (value: string, transport: string, ack: boolean) => transport === "quic" || value !== "none" || ack;

/** The cipher of a tunnel: the recommended automatic choice, AES, ChaCha, or none (with a warning and a confirmation). */
export function EncryptionChoice({
  value,
  onChange,
  transport,
  ack,
  onAck,
}: {
  value: string;
  onChange: (v: Cipher) => void;
  transport: string;
  ack: boolean;
  onAck: (v: boolean) => void;
}) {
  const { t } = useApp();
  if (transport === "quic") {
    return (
      <div className="field">
        <span className="label">{t("enc.title")}</span>
        <span className="help">{t("enc.quic")}</span>
      </div>
    );
  }
  return (
    <Choice
      label={t("enc.title")}
      value={value as Cipher}
      options={CIPHERS.map((c) => [c, t(`enc.${c}`)])}
      onChange={onChange}
      risky={value === "none"}
      hint={
        <>
          {t(`enc.${value}.d`)}
          {transport === "wss" && value !== "none" && ` ${t("enc.tls")}`}
        </>
      }
    >
      {value === "none" && (
        <div className="enc-warn" role="alert">
          <Icon name="alert" size={18} />
          <div>
            <p>{t(transport === "wss" ? "enc.noneWarn.tls" : "enc.noneWarn")}</p>
            <label className="check">
              <input type="checkbox" checked={ack} onChange={(e) => onAck(e.target.checked)} /> {t("enc.noneAck")}
            </label>
          </div>
        </div>
      )}
    </Choice>
  );
}
