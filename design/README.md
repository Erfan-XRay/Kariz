# Kariz panel design system

The visual language of the web panel: "Kariz at night". The idea and the plan are in
[docs/PHASE9.md](../docs/PHASE9.md); this document is the reference for building it.
The tokens are in [tokens.css](tokens.css); the interactive prototype is in
[prototype/](prototype/).

## 1. The metaphor, applied

| Thing in Kariz | On screen |
|---|---|
| A server | A well: a shaft from the horizon down, with a spoil mound on top |
| A tunnel | An underground channel between two wells |
| Traffic | Water particles in the channel; speed and density follow throughput |
| A healthy tunnel | Turquoise water flowing |
| A stopped tunnel | An empty channel, a dashed outline |
| A failing tunnel | Still water turning red at the break, a pulse where it fails |
| The page | A cross-section: sky above the horizon (navigation, titles), earth below (content in strata) |

## 2. Colour

Components use semantic tokens only. Raw brand colours (`--navy-*`, `--gold-*`,
`--water-*`) are for the scene art (the map, the login, the loader).

| Role | Token | Use | Do not use for |
|---|---|---|---|
| Accent | `--accent` | primary buttons, focus ring, selection, active navigation | status |
| Water | `--water` | live traffic, "connected", healthy | buttons, links |
| Warning | `--warn` | degraded, restarting, a certificate close to expiry | |
| Danger | `--danger` | down, errors, destructive actions | |
| Lines | `--line`, `--line-strong` | the hairline between strata, borders of fields | text |

Status never relies on colour alone: every state also has an icon and a word.

Contrast (WCAG ratio on `--bg` / `--stratum-1` / `--stratum-2`):

| Night | | Dawn | |
|---|---|---|---|
| `--text` #eef2f7 | 15.6 / 14.6 / 12.9 | `--text` #0b1f34 | 14.6 / 13.0 / 11.4 |
| `--text-2` #a9b8cc | 8.7 / 8.2 / 7.2 | `--text-2` #3a4f66 | 7.4 / 6.6 / 5.8 |
| `--text-3` #7f92ab | 5.5 / 5.2 / 4.6 | `--text-3` #4f6175 | 5.9 / 5.2 / 4.6 |
| `--accent` #e9c46a | 10.5 / 9.8 / 8.7 | `--accent` #7a4e00 | 6.3 / 5.6 / 4.9 |
| `--water` #34d0c3 | 9.2 / 8.6 / 7.6 | `--water` #0a6e65 | 5.4 / 4.8 / 4.2 (large text and icons on `--stratum-2`) |
| `--danger` #ff7a7a | 6.9 / 6.5 / 5.8 | `--danger` #b42318 | 5.8 / 5.1 / 4.5 |

`--on-accent` on `--accent`: 11.0 (Night), 8.2 (Dawn, with the fill #d9a441 for large
buttons) or white text on #7a4e00.

## 3. Type

| Font | Role | Why |
|---|---|---|
| Space Grotesk | UI text, headings, numbers | geometric and technical, with character; a variable font (22 KB, Latin) |
| Estedad | Persian text | a modern Persian sans with even stroke weight that sits well next to Space Grotesk; variable (125 KB) |
| IBM Plex Mono | addresses, ports, tokens, logs, config | clear `0`/`O` and `1`/`l`, calm in long logs |

Estedad is limited to Arabic script with `unicode-range`, so mixed lines like
«تانل روی 203.0.113.5:3080» set the address in Latin type. Digits follow a setting
(Persian or Latin); addresses, ports and versions are always Latin.

Scale: 12 / 14 / 16 / 20 / 28 / 40 / 64 px. Body 16 px with line height 1.55. Key numbers
use tabular figures (`font-variant-numeric: tabular-nums`) so they do not jump while they
update.

## 4. Space, shape, surfaces

- A 4 px grid: 4, 8, 12, 16, 24, 32, 48, 64.
- **Strata, not cards.** A page is a stack of full-width bands (`--stratum-1`, `-2`,
  `-3`, deeper is darker in Night and darker sand in Dawn), divided by a 1 px gold
  hairline. Within a band, content is laid out in columns, without boxes around each item.
- Small radii: 4 (chips), 8 (buttons, fields), 14 (dialogs). Bands have none.
- A faint grain (an SVG noise, `--grain-opacity`) over the earth, never over text-heavy
  areas at more than 7 %.
- Shadows only on things that float: dialogs, the command palette, menus.

## 5. Layout

- **Desktop:** a side rail (icons with labels, 76 px; 240 px when expanded) on the
  start side, mirrored in RTL. The sky strip across the top holds the page title,
  search (`Ctrl+K`) and the account. Content below the horizon.
- **Mobile (below 768 px):** the rail becomes a bottom bar of at most five items; the map
  becomes a vertical list of channels.
- Breakpoints 375, 768, 1024, 1440. No horizontal scroll at 375 px.

## 6. Motion

| Token | ms | For |
|---|---|---|
| `--dur-micro` | 120 | hover, press |
| `--dur-short` | 200 | toggles, chips, tooltips |
| `--dur-mid` | 320 | drawers, panels, list changes |
| `--dur-long` | 560 | page transitions |
| `--dur-scene` | 1200 | the login descent |

- Enter with `--ease-out`, leave with `--ease-in` and about 30 % faster.
- Water uses `--ease-water` and constant speed along a path.
- Only `transform` and `opacity` are animated; particles are drawn on a canvas.
- Everything moving has a meaning (traffic, a state change, where something went).
- `prefers-reduced-motion` and the low-power switch: particles stop (the channel shows
  a still gradient whose brightness follows throughput), transitions become short fades,
  the loader shows a static logo with a progress line.
- Animation pauses in a hidden tab and for parts of the map off screen.

## 7. Signature pieces

- **Loader:** three wells; a drop falls down one shaft, runs along the sloping channel
  and leaves as the logo's arrow, in a loop of about 1.6 s. Used at start-up (after the
  logo draws itself) and for long waits; short waits use a thin water line under the sky
  strip instead of a spinner.
- **Login:** a desert night (stars, dunes, the horizon, three wells). The form sits in
  the sky. Success: the view descends a shaft (1.2 s) into the dashboard. Failure: the
  field shakes once and the water below ripples. A one-time link signs in by itself,
  showing the descent.
- **The map:** wells on the horizon, channels between them, particles for traffic. Hover a
  channel for its numbers; click it to open the tunnel.
- **Odometer numbers:** counters roll digit by digit when they change.

## 8. Accessibility

- Keyboard reaches everything; the focus ring is `--focus-ring` and always visible.
- Touch targets at least 44 x 44 px.
- The map has a text alternative: a list of tunnels with their state and throughput.
- Live numbers update politely (`aria-live="polite"`, at most every few seconds).
- Labels above fields, errors under the field that caused them.

## 9. Fonts and licences

`fonts/` holds the three fonts (SIL Open Font License 1.1, texts next to them):
Estedad by Amin Abedi, Space Grotesk by Florian Karsten, IBM Plex Mono by IBM.
