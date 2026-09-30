# Phase 15 plan: 1.0

Target release: **v1.0.0**. Scope: what is needed before the project promises to keep working the
way it does: a security review with its findings fixed, tests that use the panel the way a person
does (in a real browser), accessibility, the documentation in both languages, and a promise about
compatibility from 1.0 on.

## 1. Security review (15.1)

The review is [security-review.md](security-review.md): who is assumed to attack, what was checked
(sign-in, CSRF, injection into files, names as paths, what the agent may run, agent identity,
updates, secrets, the web app, dependencies), what was found and changed, and the limits that are
left and said so. Each finding that was fixed has a test. `cargo audit` runs in CI.

## 2. Browser tests (15.2)

Playwright, against a real panel and a real agent on the CI runner (systemd is not needed:
`services = "process"` runs the tunnels as child processes), in Chromium:

- sign in with a password, with a link, and the lockout after five wrong tries;
- add a server with a join code and see it come online;
- make a tunnel with the wizard, send traffic through it, edit it, stop it, delete it, and see a
  tunnel that cannot connect leave nothing behind;
- the pages in both languages and both themes, on a phone and on a desktop;
- the update dialog against a local server of releases.

## 3. Accessibility (15.3)

axe-core runs on every page and dialog, in both themes and both languages (contrast, names, roles,
focus); the keyboard reaches everything, dialogs trap and give back focus, and animation stops under
`prefers-reduced-motion` and in the low-power switch. What axe cannot judge is checked by hand and
listed.

## 4. Documentation in both languages (15.4)

The README and the guides a user needs to set Kariz up and run it are in Persian as well as English
(`docs/fa/`): getting started, the manager, the panel, private networks, security, troubleshooting.
The reference pages (configuration, transports, the status document) stay in English, which is what
their words are in.

## 5. Compatibility from 1.0 (15.5)

[compatibility.md](compatibility.md) says what 1.0 promises: the configuration format, the
control-socket status document, the panel's link protocol between versions that are one minor version
apart, and how a breaking change is announced. Then the release.

## 6. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **15.0** Plan | This document. | |
| **15.1** Security review (done) | The review, its fixes and their tests, `cargo audit` in CI. | The review is in the repository and CI is green. |
| **15.2** Browser tests (done) | Playwright and its CI job. | The flows above pass in CI. |
| **15.3** Accessibility (done) | axe on every page, the keyboard, motion. | No violation left, or each one explained. |
| **15.4** Documentation | The Persian guides. | Both languages say the same things. |
| **15.5** Release | `compatibility.md`, CHANGELOG, `1.0.0`. | CI green; release. |
