import { defineCollection } from "astro:content";
import { glob } from "astro/loaders";

// One source: the markdown files of `docs/` (and `docs/fa/`), and the changelog. The site
// adds no copy of any of them.
const docs = defineCollection({
  loader: glob({ pattern: ["*.md", "!README.md"], base: "../docs", generateId: ({ entry }) => entry.replace(/\.md$/, "").toLowerCase() }),
});
const fa = defineCollection({
  loader: glob({ pattern: ["*.md", "!README.md"], base: "../docs/fa", generateId: ({ entry }) => entry.replace(/\.md$/, "").toLowerCase() }),
});
const root = defineCollection({
  loader: glob({ pattern: ["CHANGELOG.md"], base: "..", generateId: ({ entry }) => entry.replace(/\.md$/, "").toLowerCase() }),
});

export const collections = { docs, fa, root };
