// Reading the documentation collections: which pages exist in which language, their
// titles, and the neighbours of a page.
import { getCollection, render } from "astro:content";
import type { CollectionEntry } from "astro:content";
import { LABELS, OWN_PAGES, SECTIONS, ORDER } from "./site";
import type { Lang } from "./site";

type Entry = CollectionEntry<"docs"> | CollectionEntry<"fa"> | CollectionEntry<"root">;

export interface Doc {
  slug: string;
  /** The language the text is written in. */
  written: Lang;
  entry: Entry;
}

let cache: Promise<{ en: Map<string, Entry>; fa: Map<string, Entry> }> | null = null;

async function load() {
  const [docs, root, fa] = await Promise.all([getCollection("docs"), getCollection("root"), getCollection("fa")]);
  const en = new Map<string, Entry>([...docs, ...root].map((e) => [e.id, e]));
  return { en, fa: new Map<string, Entry>(fa.map((e) => [e.id, e])) };
}
const all = () => (cache ??= load());

/** Every slug the site has a page for: the files of docs/, the changelog, and the site's own pages. */
export async function slugs(): Promise<string[]> {
  const { en } = await all();
  const known = new Set(ORDER);
  const missing = [...en.keys()].filter((s) => !known.has(s));
  if (missing.length) {
    throw new Error(`docs/ has pages that are not in the site map (site/src/lib/site.ts SECTIONS): ${missing.join(", ")}`);
  }
  return [...en.keys()];
}

/** The page in `lang`; falls back to the English text (the page then says so). */
export async function getDoc(slug: string, lang: Lang): Promise<Doc | null> {
  const { en, fa } = await all();
  if (lang === "fa" && fa.has(slug)) return { slug, written: "fa", entry: fa.get(slug)! };
  const e = en.get(slug);
  return e ? { slug, written: "en", entry: e } : null;
}

export async function hasTranslation(slug: string, lang: Lang): Promise<boolean> {
  if (OWN_PAGES.has(slug) || lang === "en") return true;
  return (await all()).fa.has(slug);
}

export async function rendered(doc: Doc) {
  return render(doc.entry as CollectionEntry<"docs">);
}

/** First `# ` heading of the source. */
export function h1(body: string | undefined): string {
  return /^#\s+(.+)$/m.exec(body ?? "")?.[1].trim() ?? "";
}

export async function titleOf(slug: string, lang: Lang): Promise<string> {
  if (LABELS[slug]) {
    // A translated page names itself; a page not yet translated keeps the English label.
    return LABELS[slug][(await hasTranslation(slug, lang)) ? lang : "en"];
  }
  const d = await getDoc(slug, lang);
  const t = h1(d?.entry.body);
  return t || slug;
}

/** The first paragraph, plain, for cards and meta descriptions. */
export function summary(body: string | undefined, max = 140): string {
  const text = (body ?? "")
    .split(/\n\s*\n/)
    .map((p) => p.trim())
    .find((p) => p && !/^(#|```|\||>|<|-|\*|\[English\]|\[فارسی\])/.test(p));
  const plain = (text ?? "")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[`*_]/g, "")
    .replace(/\s+/g, " ");
  return plain.length > max ? `${plain.slice(0, max).replace(/\s+\S*$/, "")}…` : plain;
}

export async function neighbours(slug: string) {
  const i = ORDER.indexOf(slug);
  return { prev: i > 0 ? ORDER[i - 1] : null, next: i >= 0 && i < ORDER.length - 1 ? ORDER[i + 1] : null };
}

export async function sidebar(lang: Lang) {
  return Promise.all(
    SECTIONS.map(async (s) => ({
      id: s.id,
      title: s.title[lang],
      pages: await Promise.all(
        s.pages.map(async (slug) => ({ slug, title: await titleOf(slug, lang), translated: await hasTranslation(slug, lang) })),
      ),
    })),
  );
}
