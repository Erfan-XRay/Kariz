/** What a tunnel's last error likely means: the same list as the panel's `src/cause.rs`, so
 * the page and the Telegram message say the same thing. Null for an error not known. */
export type Cause = "unreachable" | "refused" | "handshake" | "silent" | "reset" | "closed" | "no_answer" | "auth";

export function causeOf(error: string): Cause | null {
  const e = error.toLowerCase();
  const has = (...words: string[]) => words.some((w) => e.includes(w));
  // The more particular texts first: "connect to … timed out" and "handshake timed out" are timeouts too.
  if (has("authentication failed")) return "auth";
  if (has("no answer through the tunnel", "did not answer in time")) return "no_answer";
  if ((e.startsWith("connect to ") && e.includes("timed out")) || has("no route to host", "network is unreachable", "os error 113", "os error 101")) return "unreachable";
  if (has("handshake timed out")) return "handshake";
  if (has("connection refused", "os error 111")) return "refused";
  if (has("connection reset", "os error 104", "broken pipe", "os error 32")) return "reset";
  if (has("os error 110", "operation timed out", "stopped answering pings")) return "silent";
  if (has("peer closed", "unexpected end of file", "early eof")) return "closed";
  return null;
}
