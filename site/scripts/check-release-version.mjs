// The archive names in docs/ (kariz-v2.8.5-x86_64-linux.tar.gz) must be the version in
// Cargo.toml, so the guides read on GitHub are as new as the site (which rewrites them by
// itself: src/plugins/remark-release-version.mjs). A stable release bump that forgets them
// fails here. A beta is not checked: the guides point at the latest stable release.
import fs from "node:fs";
import path from "node:path";

const root = new URL("../../", import.meta.url);
const version = /^version = "([^"]+)"/m.exec(fs.readFileSync(new URL("Cargo.toml", root), "utf8"))?.[1] ?? "";
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  console.log(`release version: ${version} is a prerelease, not checked`);
  process.exit(0);
}
const ARCHIVE = /kariz-v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)-(?:x86_64|aarch64|armv7)-linux/g;
const wrong = [];
for (const dir of ["docs", "docs/fa"]) {
  const abs = new URL(`${dir}/`, root);
  for (const name of fs.readdirSync(abs)) {
    if (!name.endsWith(".md") || name === "CHANGELOG.md") continue;
    const text = fs.readFileSync(new URL(name, abs), "utf8");
    for (const m of text.matchAll(ARCHIVE)) {
      if (m[1] !== version) wrong.push(`${path.posix.join(dir, name)}: ${m[0]}`);
    }
  }
}
if (wrong.length) {
  console.error(`These archive names are not Kariz ${version} (Cargo.toml):\n  ${wrong.join("\n  ")}`);
  process.exit(1);
}
console.log(`release version: the docs name Kariz ${version}`);
