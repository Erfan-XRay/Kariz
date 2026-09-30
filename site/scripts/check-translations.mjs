// A page missing in one language must be said out loud: every English page of docs/ that has
// no Persian version is listed in untranslated.txt, and the list must be true. A new page
// without a translation (or a translation that arrives) fails here until the list is updated.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const site = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const docs = path.join(site, "../docs");
const slugs = (dir) =>
  fs.readdirSync(dir).filter((f) => f.endsWith(".md") && f !== "README.md").map((f) => f.replace(/\.md$/, "").toLowerCase());

const en = new Set(slugs(docs));
const fa = new Set(slugs(path.join(docs, "fa")));
const listed = new Set(
  fs.readFileSync(path.join(site, "untranslated.txt"), "utf8").split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith("#")),
);

const errors = [];
for (const s of fa) if (!en.has(s)) errors.push(`docs/fa/${s}.md has no English page (docs/${s}.md)`);
for (const s of en) {
  if (!fa.has(s) && !listed.has(s)) errors.push(`docs/${s}.md has no Persian version: translate it in docs/fa/, or add "${s}" to site/untranslated.txt`);
}
for (const s of listed) {
  if (!en.has(s)) errors.push(`site/untranslated.txt lists "${s}", which is not a page of docs/`);
  else if (fa.has(s)) errors.push(`site/untranslated.txt lists "${s}", but docs/fa/${s}.md exists: remove it from the list`);
}
if (errors.length) {
  for (const e of errors) console.error(`error: ${e}`);
  process.exit(1);
}
console.log(`translations: ${fa.size} pages in Persian, ${listed.size} listed as English only, ${en.size} in all`);
