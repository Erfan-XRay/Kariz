# Security review of the panel (for 1.0)

This is what was checked before 1.0, what was found, what was changed, and what is left as a
known limit. It covers the panel, its agents and its update path; the tunnel core has its own notes
in [security.md](security.md).

## Who is assumed to attack

| Attacker | Can | Cannot (by design) |
|---|---|---|
| Someone on the internet who finds the panel's port | send any request, open many connections, try passwords and links | learn that it is a panel (any address but the secret path is nginx's plain 404), or try more than 5 wrong secrets per address in 15 minutes |
| A web page the admin visits while signed in | make the browser send requests | act: the cookie is `SameSite=Strict`, every change needs a header only the app knows, and a page cannot send `application/json` without the browser asking first |
| A server that has been taken over (an agent) | answer the panel with anything, drop the link | make the panel run anything, or reach other servers: the panel only reads its answers as data |
| The network between the panel and a server | read and change bytes | read or change anything useful: the link is Kariz's own (encrypted, and both sides prove keys) |
| A poisoned download | offer a program | have it run: releases are signed, and a file whose signature does not verify is deleted |
| Someone with a copy of the database | read it | sign in or impersonate a server without more: secrets are stored as hashes (Argon2id, BLAKE3); the agents' keys are in it, so it is kept `0600` in a `0700` folder |

**What a panel compromise is worth.** Whoever controls the panel controls the tunnels on every
server it manages (make, stop, delete, read logs) and can put the servers on a private network. It
cannot run commands there and cannot install a program that was not signed by the release key.
That is the price of a management panel; it is why the address is secret, the port is random, and
the sign-in is hardened.

## What was checked

| Area | Result |
|---|---|
| Sign-in: password hashing, link handling, lockout, sessions | Argon2id (19 MiB, 2 passes), links and sessions stored as hashes only, a link works once, sessions listed and revocable, cookie `__Host-`, `Secure`, `HttpOnly`, `SameSite=Strict`. **OK.** |
| CSRF | the header `X-Kariz-CSRF` on every change plus a JSON body plus `SameSite=Strict`. **OK.** |
| Injection into the tunnel files | every field of a tunnel is a typed value; text with a control character is refused; the file is rendered by the agent, never taken from the panel. A test found that a newline in an address was accepted before phase 12 ended; fixed and tested. **OK.** |
| Names as paths | tunnel and interface names are `[a-z0-9-]` with a length limit; versions are checked before they become a folder. Tests for `../` and friends. **OK.** |
| Commands run by the agent | a fixed list: `systemctl`, `journalctl`, `ip`, `ping`, `kariz speedtest`, with fixed arguments and checked values, and only `kariz@NAME` units and `kz-` interfaces. No shell. **OK.** |
| Agent identity | a link token plus a per-agent key, proved over a random challenge with a keyed hash compared in constant time; a join code works once. **OK.** |
| Updates | Ed25519 signatures over a checksum file that names the archive; verified by the panel, again by every agent, and by `kariz-manager`; a rollback if the new program does not come up. **OK**; see the changes below for the download itself. |
| Secrets in answers | tunnel tokens are never sent to the panel's browser or kept by the panel; the backup is encrypted. **OK.** |
| Web app | no `innerHTML` or `dangerouslySetInnerHTML`; text from servers (logs, release notes, names) is always rendered as text. **OK.** |
| Dependencies | `cargo audit` runs in CI on every change. |

## What was found and changed

1. **No limits on idle connections.** A client could open connections and say nothing until the
   panel ran out of sockets. Now the TLS handshake must finish in 10 seconds, a request's headers
   in 15, and there are at most 512 connections (another is closed at once). Tested with real TLS.
2. **The API's answers had none of the browser hardening headers** (only the web app's files did).
   Every answer under the secret path now has `Cache-Control: no-store`, `X-Content-Type-Options`,
   `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, a Content-Security-Policy (`default-src
   'none'` for the API; for the app, scripts, connections, fonts and styles only from itself, no
   objects, no framing), HSTS, and cross-origin and permissions policies. The 404 for any other
   address is left exactly as nginx's.
3. **The database's permissions depended on whoever started the panel.** It holds the agents' keys.
   It is now `0600` in a `0700` folder whatever the umask.
4. **A release listing could have pointed the download at a service on the same server**
   (`http://127.0.0.1:...`; only the tests need that). Downloads follow `https` addresses only,
   unless the repository itself was configured as a local one.
5. **An archive could hold thousands of files or unpack to a huge size.** Now at most 500 entries
   and 400 MB, and only the two programs are ever read from it.
6. **The audit log grew for ever.** It is now kept for a year.
7. **Password checks were unlimited in number at a time.** Each costs real memory and CPU, so a
   flood from many addresses could make the panel unusable. At most four run at once; others get
   `429`.
8. **A tunnel with many IPv6 ports was silently refused** (a request was limited to 16 KB, and 200
   forwards can be over 20 KB). The limit is 60 KB (a stream's open bytes hold 64 KB), the API
   takes 128 KB for the calls that make a tunnel, and a request that is too large says so.

## Known limits (accepted, and said so)

- **The lockout is per address.** Someone with many addresses (an IPv6 /64) can try more; the
  Argon2 cost, the gate of four checks at a time, and a password of at least 10 characters are what
  stand against that. Use a long password, or only sign in with links.
- **The certificate is self-signed by default,** so the first visit trusts what the installer
  printed (compare the fingerprint). Use your own certificate (`cert_file`) to remove that step.
- **The panel says whether a password is set** before sign-in (the login page needs to know).
- **The agent runs as root** because it manages services and interfaces. Its requests are the fixed
  list above, but a bug in one of them is a bug as root; they are small, validated and tested.
- **GRE itself is not encrypted;** Kariz encrypts what runs inside it. See [networks.md](networks.md).
- **Backups hold the agents' keys** (that is what lets a restored panel talk to them); they are
  encrypted with a passphrase you choose. Keep both safe.
- **The release key is one key.** If it leaks or is lost, releases cannot be trusted or made: a new
  key means a new program with a new built-in key, installed by hand once.

## How to report a problem

Open an issue on the repository, or write to the maintainer privately if it should not be public
yet. Please include the version (`kariz-panel --version`).
