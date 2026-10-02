// Copies what the site shares with the rest of the repository into places Astro can serve:
// the panel's tokens and fonts, and the interactive prototype (the live demo). Nothing here
// is committed; the repository has one copy of each.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const site = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repo = path.resolve(site, "..");
const rm = (p) => fs.rmSync(p, { recursive: true, force: true });
const cp = (a, b) => fs.cpSync(a, b, { recursive: true });

// Tokens and fonts for the site's own styles.
const panel = path.join(site, "src/styles/panel");
rm(panel);
fs.mkdirSync(panel, { recursive: true });
cp(path.join(repo, "panel/web/src/tokens.css"), path.join(panel, "tokens.css"));
cp(path.join(repo, "panel/web/src/fonts"), path.join(panel, "fonts"));

// The panel's drawing helpers, which the hero scene shares with the panel's own scenes.
const scripts = path.join(site, "src/scripts/panel");
rm(scripts);
fs.mkdirSync(scripts, { recursive: true });
cp(path.join(repo, "panel/web/src/draw.ts"), path.join(scripts, "draw.ts"));

// The live demo: the prototype with the same tokens, fonts and logo, served as is.
const demo = path.join(site, "public/demo");
rm(demo);
cp(path.join(repo, "design/prototype"), demo);
cp(path.join(repo, "panel/web/src/tokens.css"), path.join(demo, "tokens.css"));
cp(path.join(repo, "panel/web/src/fonts"), path.join(demo, "fonts"));
cp(path.join(repo, "assets/logo.svg"), path.join(demo, "logo.svg"));
const index = path.join(demo, "index.html");
fs.writeFileSync(
  index,
  fs
    .readFileSync(index, "utf8")
    .replaceAll('"../tokens.css"', '"tokens.css"')
    .replaceAll('"../../assets/logo.svg"', '"logo.svg"'),
);
// The prototype opens in Persian and Night; the site passes ?lang=en and ?theme=dawn along.
const app = path.join(demo, "app.js");
const src = fs.readFileSync(app, "utf8");
const from = /^(\s*)lang: "fa",\r?\n\s*theme: "night",/m;
if (!from.test(src)) throw new Error("design/prototype/app.js changed: update the start-up patch in site/scripts/prepare.mjs");
const query = 'new URLSearchParams(location.search)';
fs.writeFileSync(
  app,
  src.replace(
    from,
    (_, ind) =>
      `${ind}lang: ${query}.get("lang") === "en" ? "en" : "fa",\n${ind}theme: ${query}.get("theme") === "dawn" ? "dawn" : "night",`,
  ),
);
cp(path.join(repo, "assets/logo.svg"), path.join(site, "public/logo.svg"));
// The card that link previews show (the repository's social image).
cp(path.join(repo, "assets/social-preview.png"), path.join(site, "public/social-preview.png"));
console.log("site assets ready");
