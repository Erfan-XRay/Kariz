# Accessibility of the panel

What is checked by a program on every change, and what a person has to check.

## Checked automatically (every change, in CI)

`panel/web/e2e/04-a11y.spec.ts` runs [axe-core](https://github.com/dequelabs/axe-core) in a real
browser against WCAG 2.1 levels A and AA on:

- every page (Map, Servers, Tunnels, Networks, Logs, Settings) in **each of the four looks**:
  English and Persian, Night and Dawn;
- the sign-in page;
- the dialogs (add a server, the password, the command palette) and the tunnel pages;
- that the keyboard reaches every part of the rail, that focus **stays inside a dialog** while
  tabbing and goes back to what opened it, and that a visible focus ring exists;
- that the page's low-power switch turns on by itself when the browser asks for less motion
  (`prefers-reduced-motion`).

Zero violations is the bar. When axe reported something, it was fixed rather than switched off:
the text colour for secondary text was raised in both themes to 4.5:1 or more on every surface it
sits on, the map and the page area got names and keyboard access, the command palette became a
proper combobox with a named list, and dialogs, the wizard and the palette now keep Tab inside
themselves (they did not before).

## What the design does on purpose

- **Every animation stops** under `prefers-reduced-motion` and with the low-power switch (the leaf
  in the top bar); neither is needed to use anything.
- **Colour is never the only sign.** A tunnel's state is written next to its dot, a failed step has
  a cross and the words, and traffic on the map is also in the table under it.
- **Persian is not a translation added after:** the page turns right to left (`dir="rtl"`), code,
  addresses and paths stay left to right inside Persian text, and digits follow the setting.
- **Touch targets** are 40 px or larger; on a phone nothing scrolls sideways (a test walks every
  page at 375 px in both languages).
- Text uses real headings, landmarks (`nav`, `main`, `header`), and labels tied to their fields;
  errors are announced (`role="alert"`), and a running operation is a live region.

## Checked by hand (what a program cannot judge)

Before each release, someone goes through these with the keyboard alone and with a screen reader
(NVDA or VoiceOver) on the Persian and the English page:

1. Sign in with a link and with a password; the error messages are read out.
2. Make a tunnel with the wizard; each step's heading and each checklist line is read out as it
   completes.
3. The map: the text under it (`map-alt`) says what is on it in a way that makes sense without
   seeing it.
4. Dialogs open with focus on something sensible and close back to where they were.

If you find something that fails, open an issue with the page, the language and what you used.
