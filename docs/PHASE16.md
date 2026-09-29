# Phase 16 plan: the documentation website

Target: right after **v1.0.0** is out, on the repository's own GitHub address (GitHub Pages:
`https://erfan-xray.github.io/Kariz/`). Not a release of the program: a site that is built
and published from the repository, and republished by every release.

## 1. What it is

The documentation of Kariz (today the files in `docs/`, both READMEs and the CHANGELOG) as
a website that looks and feels like the project: the same night-desert language as the
panel, with animation that means something, in Persian and English.

## 2. Look and feel ("Kariz at night", from `design/`)

- **The hero is the qanat:** a night sky, the horizon, wells, and a channel of water that
  really flows (canvas, the code of the panel's login scene reused). Scroll moves the camera
  down the well; each section of the front page is a stratum of the earth, the way the panel
  is laid out, with the same gold hairlines.
- **The tokens and fonts of the panel** (Space Grotesk, Estedad, IBM Plex Mono, self-hosted),
  Night and Dawn themes, Persian (RTL) and English (LTR), Persian digits by choice.
- **Motion with a meaning:** water that fills as a page loads, a drop that draws the reading
  progress on long pages, code blocks that copy with a small splash. Every animation stops
  under `prefers-reduced-motion` and in a low-power switch, as in the panel.
- **A live demo of the panel:** the interactive prototype of phase 9 (`design/prototype/`),
  fed with sample data, runs in the browser from a "Try the panel" button. It is the same
  screens as the real one; nothing is sent anywhere.
- **Diagrams that move:** how a tunnel works (layers, direct and reverse, the mux), which
  transport to pick (an interactive chooser), what each profile changes.

## 3. Structure

| Section | From |
|---|---|
| Home: what Kariz is, the numbers, the panel, install in one line | README |
| Get started (install, a first tunnel, the panel) | `getting-started.md`, `manager.md`, `panel.md` |
| Concepts (layers, transports, profiles, UDP and games, CDN) | `transports.md`, `profiles.md`, `udp-and-games.md`, `CDN.md` |
| Guides (private networks, updating, backups, troubleshooting) | `networks.md`, `troubleshooting.md`, ... |
| Reference (every setting, the status document, the CLI) | `configuration.md`, `status.md` |
| Security | `security.md` |
| Changelog and roadmap | `CHANGELOG.md`, `ROADMAP.md` |

The pages are the markdown files of `docs/`: **one source**. The site is built from them (no
second copy to keep in step), with extra front matter only where a page needs it. Search
runs in the browser over an index built at publish time (Pagefind), so it needs no server
and works offline once loaded.

## 4. Build and publish

- **Generator:** Astro (the same choice as XRayMesh's site, static output, islands for the
  few interactive parts), TypeScript, no client framework outside the demo.
- **Where it lives:** `site/` in this repository; `docs/` stays the source of the text.
- **CI:** a workflow builds the site on every pull request (it must build, links must not
  break, a check for missing translations) and publishes to Pages on a release tag and on
  pushes to `main` that touch `docs/` or `site/`.
- **Size and speed:** static files, fonts self-hosted, images as SVG or WebP, the canvas
  code loaded only on the pages that use it; a page must be usable before any script runs.

## 5. Accessibility and language

Real headings and landmarks, keyboard reach everywhere, contrast as measured for the panel's
tokens, RTL correct in code blocks (they stay left-to-right), a language switch that keeps
the page. Persian is a first-class version, not a translation added later: a page missing in
one language says so.

## 6. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **16.0** Plan | This document. | |
| **16.1** Design | The site's screens designed from the panel's system: front page, a docs page, search, the demo. | Approved. |
| **16.2** Build | Astro project, the theme, the docs pages from `docs/`, search, both languages. | The site builds from `docs/` alone. |
| **16.3** The hero and the demo | The qanat hero and scroll, the live demo of the panel, the diagrams. | Smooth at 60 fps on a mid phone; reduced motion honoured. |
| **16.4** Publish | The Pages workflow, link and translation checks, the README pointing to it. | The site is live at the repository's Pages address. |

## 7. Depends on

The repository being **public** (GitHub Pages on a private repository needs a paid plan),
which is also what the panel's self-update needs to see releases without a token.
