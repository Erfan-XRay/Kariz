# Phase 14 plan: updating from the panel

Target release: **v0.11.0**. Scope: when a new release is out, the panel shows it, and one
button updates the panel, and (with a second, explicit button) every connected server: their
Kariz programs and their agents, with the tunnels restarted one at a time.

## 1. Why this is designed the way it is

- **Servers that cannot reach GitHub are the normal case** here. So the panel downloads a
  release *once* and gives it to its agents over the link that already exists; an agent never
  needs internet access to update.
- **The update replaces programs that run as root**, so it must not trust more than a
  checksum from the same place as the file. Releases are **signed**, and the programs
  check the signature (section 3).
- **An update that breaks the panel must not strand the user**: the old program is kept, the
  new one has to prove it starts, and if it does not, the old one is put back by itself.

## 2. What the user sees

- A quiet notice in the top bar when a newer release exists: `Kariz 0.11.0 is out`, with what
  changed (the CHANGELOG entry) and two buttons:
  1. **Update the panel** (this server).
  2. **Update all servers** (after the panel: each server in turn, the tunnels on it restarted
     one at a time so a pair never loses both sides at once; the map shows the progress).
- **Settings > Updates:** the installed versions per server, *Check now*, a daily automatic
  check on or off, and the channel: **stable** (default) or **beta** (prereleases).
- Every step is shown as it happens and written to the audit log; an update can be cancelled
  before it starts, and a failed server is left as it was, with the reason.
- The panel never updates by itself: the check is automatic, the update is a button.

## 3. Trust: signed releases

- The release workflow signs each archive's SHA-256 with an **ed25519 key** kept as a GitHub
  secret; the signature is a release asset (`kariz-vX.Y.Z-<arch>-linux.tar.gz.sig`).
- The **public key is built into** `kariz-panel` and `kariz-manager` (and can be pinned in
  `panel.toml` by the user). A file whose signature does not verify is deleted, never run.
- **No downgrades** through the update button, and a release older than the installed one is
  refused; a newer major version asks for confirmation.
- The panel says where it got the release from and the checksum it verified, in the audit
  log.

This needs a key pair the owner makes once (`kariz-panel release-key`), with the private half
stored only as a repository secret; see the decisions at the end.

*Status after 14.1:* `kariz-panel release-key --out FILE` makes the key pair (the private half goes
to the file, mode 0600, and becomes the repository secret `KARIZ_SIGNING_KEY`), `release-sign
X.tar.gz.sha256` writes `X.tar.gz.sig` (64 raw bytes: the signature of the `.sha256` file's exact
bytes, which also binds the archive's name), and `release-verify X.tar.gz` checks archive,
checksum file and signature with the built-in key (or `--key HEX`). The release workflow signs
all three archives and checks each signature with the key built into the program before it
publishes, so a wrong or missing secret stops the release. `kariz-manager install|update`
verifies with `openssl pkeyutl -verify -rawin` and the key in the script: releases before 0.11
have no signature and are accepted with a warning, from 0.11 a missing or wrong signature stops
the installation (`KARIZ_RELEASE_KEY_PEM` pins another key). The public key is
`586c5c36...8d3b`; the tests cover a wrong key, a damaged signature, a checksum file changed
after signing, another archive's bytes, an archive of another CPU, and a missing signature.

## 4. How the update happens

1. **Find:** `GET https://api.github.com/repos/Erfan-XRay/Kariz/releases/latest` (or the list,
   for the beta channel), through the panel's own connection. Works when the repository is
   public, or with a read token set in `panel.toml`. The result is cached; a failure is shown
   and retried, never an error page.
2. **Download and verify:** the archive for the panel's architecture, its checksum and
   signature. Kept in `/var/lib/kariz-panel/updates/`.
3. **Panel:** the new `kariz-panel` and `kariz` are put beside the old ones, and a small
   transient systemd unit (`systemd-run`) swaps them and restarts `kariz-panel`, so the swap
   survives the panel stopping itself. The old binaries are kept as `.previous`.
4. **Proof it works:** the new panel must answer on its own address within 30 seconds; the
   helper checks it. If not, the old binaries are restored and restarted, and the failure is
   recorded. The browser reconnects by itself.
5. **Servers:** for each agent, in order, the panel sends the archive over the link (in
   pieces, with flow control like any stream); the agent verifies it, replaces its programs,
   restarts its tunnels one at a time (waiting for each to reconnect, `kariz status` says
   when), and restarts itself; the panel waits for it to come back on the new version. Any
   step failing puts that server back and the rest wait for the user.
6. **What is restarted:** tunnels do drop for a moment. The panel says so before it starts
   and lets the user choose the time.

New agent requests (data only, as always): `update_offer {version, size, sha256, signature}`,
`update_chunk`, `update_apply {version}`, `update_status`. The agent writes only into its own
updates directory and to the two program paths.

*Status after 14.2 to 14.4:* `panel/src/update.rs` finds a release (GitHub's `/releases`, drafts left
out, stable or beta), downloads it with `ureq`, checks it (`sign.rs`), unpacks only `kariz` and
`kariz-panel`, and swaps them with `apply` (copies to `.previous`, renames the new ones in, restarts,
asks, rolls back); `updater.rs` runs it as an operation and starts the helper with `systemd-run` as
the transient service `kariz-panel-update`, which starts three seconds late so the browser sees the
hand-over went well. The helper is the **new** program, and it asks the panel for its version over TLS
with the panel's own certificate pinned. For the agents, `agent_update.rs` receives the release (a
request holds 16 KB, so it comes in pieces of 9000 bytes, six requests at once, each checked for offset
and size), verifies it again with the agent's own key, and starts `kariz-agent-update`; the helper
counts the new agent as working when it has reached the panel again (the agent touches `connected`
beside `agent.toml`), else it puts the old programs back. Changes from the plan: the panel does not
send `update_offer`/`update_status`; three requests carry the transfer (`update_begin`,
`update_chunk`, `update_apply`) and the panel learns the result from the agent coming back with the
new version in its `hello`. A too-old agent answers "unknown request" and is reported as such. CI
(`tests/update_e2e.sh`, in the manager job, on a real systemd host) builds a second panel that
calls itself 99.0.0 (`KARIZ_BUILD_VERSION`), serves signed releases from a local server, and checks
a release that does not come up being rolled back, a good release taking, and the agent being updated
over the link with its tunnels restarted one at a time. The owner needs the repository secret
`KARIZ_SIGNING_KEY` (the private half of the key) before a release can be published.

## 5. Old and new versions together

The panel and its agents may be at different versions for a while (a server that was offline).
Every request and reply carries fields the other side may ignore; the `hello` reply says which
requests an agent understands; the panel shows a server whose agent is too old to be updated
by the panel, with the one command to update it by hand (`kariz-manager update`).

## 6. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **14.0** Plan | This document. | |
| **14.1** Signing (done) | Key generation, the workflow signs the archives, verification in the panel and the manager; tests with a good, a wrong and a damaged signature. | An archive with a bad signature is refused everywhere. |
| **14.2** The panel (done) | Finding a release, the download, verification, the swap helper with rollback; Settings > Updates and the top-bar notice. | A CI test updates a panel from one build to the next and rolls back a broken one. |
| **14.3** Servers (done) | The agent requests, the rolling update with tunnel restarts, progress on the map. | A CI test updates a panel and an agent on one host and sees both on the new version. |
| **14.4** Release (done) | Docs, CHANGELOG, `0.11.0`. | CI green; release, and the first update made with the button. |
