// Assembles the GitHub Pages output into `_site/` at the repo root.
//
//   _site/index.html          landing page (site/index.html)
//   _site/styles.css          shared stylesheet (landing + docs)
//   _site/theme.js            shared theme toggle
//   _site/assets/             docs/demo.gif, docs/web-dashboard.png
//   _site/docs/*.html         markdown rendered with `marked`
//   _site/demo/               the Web Lens demo build (dist-demo/)
//
// `marked` is a devDependency of THIS build only (site/package.json) and runs
// exclusively here, at build time — it never enters the app bundle that gets
// embedded in the Rust binary.
//
// Run `npm --prefix site ci && npm --prefix site run build` from the repo root
// (the demo build must have run first — see .github/workflows/pages.yml).

import { cp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { marked } from "marked";

const siteDir = dirname(fileURLToPath(import.meta.url));
const repo = join(siteDir, "..");
const out = join(repo, "_site");

const REPO_URL = "https://github.com/dog-hero/pg_lens";
const REPO_BLOB = `${REPO_URL}/blob/main`;

/** Markdown docs to render. `src` is repo-relative. */
const DOCS = [
  {
    src: "docs/connecting.md",
    slug: "connecting",
    title: "Connecting pg_lens to PostgreSQL",
  },
  {
    src: "docs/connection-user.md",
    slug: "connection-user",
    title: "The pg_lens monitoring role",
  },
  { src: "CHANGELOG.md", slug: "changelog", title: "Changelog" },
];

function page({ title, body, depth }) {
  // `depth` = how many directories deep the page sits, so the shared assets
  // resolve with plain relative URLs (the site lives under /pg_lens/).
  const up = "../".repeat(depth);
  return `<!doctype html>
<html lang="en" data-theme="dark">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <meta name="color-scheme" content="dark light" />
    <title>${title} — pg_lens</title>
    <link rel="stylesheet" href="${up}styles.css" />
    <script>
      try {
        var t = localStorage.getItem("pg_lens_theme");
        document.documentElement.dataset.theme = t === "light" ? "light" : "dark";
      } catch (e) {}
    </script>
  </head>
  <body>
    <svg aria-hidden="true" style="position: absolute; width: 0; height: 0; overflow: hidden">
      <symbol id="icon-lens" viewBox="0 0 24 24">
        <circle cx="11" cy="11" r="7" fill="none" stroke="currentColor" stroke-width="2" />
        <line x1="16.2" y1="16.2" x2="21" y2="21" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
      </symbol>
      <symbol id="icon-sun" viewBox="0 0 24 24">
        <circle cx="12" cy="12" r="4.5" fill="none" stroke="currentColor" stroke-width="2" />
        <g stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <line x1="12" y1="1.5" x2="12" y2="4.5" /><line x1="12" y1="19.5" x2="12" y2="22.5" />
          <line x1="1.5" y1="12" x2="4.5" y2="12" /><line x1="19.5" y1="12" x2="22.5" y2="12" />
          <line x1="4.6" y1="4.6" x2="6.7" y2="6.7" /><line x1="17.3" y1="17.3" x2="19.4" y2="19.4" />
          <line x1="4.6" y1="19.4" x2="6.7" y2="17.3" /><line x1="17.3" y1="6.7" x2="19.4" y2="4.6" />
        </g>
      </symbol>
      <symbol id="icon-moon" viewBox="0 0 24 24">
        <path d="M20 14.5A8.5 8.5 0 1 1 9.5 4a6.8 6.8 0 0 0 10.5 10.5Z" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" />
      </symbol>
    </svg>
    <header class="site-top">
      <div class="wrap">
        <a class="brand" href="${up}">
          <svg aria-hidden="true"><use href="#icon-lens"></use></svg>
          pg_lens
        </a>
        <nav>
          <a href="${up}#features">Features</a>
          <a href="${up}#install">Install</a>
          <a href="${up}demo/">Live demo</a>
          <a href="${REPO_URL}">GitHub</a>
          <button id="theme-toggle" class="theme-toggle" type="button" aria-label="Toggle light / dark theme" title="Toggle light / dark theme">
            <svg aria-hidden="true"><use id="theme-icon" href="#icon-moon"></use></svg>
          </button>
        </nav>
      </div>
    </header>
    <main class="doc">
      <div class="wrap">
        <a class="doc-back" href="${up}">← pg_lens</a>
${body}
      </div>
    </main>
    <footer>
      <div class="wrap">
        <span>MIT licensed · PostgreSQL 13+</span>
        <nav>
          <a href="${up}">Home</a>
          <a href="${REPO_URL}#readme">Full README</a>
          <a href="${REPO_URL}">GitHub</a>
        </nav>
      </div>
    </footer>
    <script src="${up}theme.js"></script>
  </body>
</html>
`;
}

/** Point the docs' repo-relative markdown links at the right place: a link
 * from one rendered document to another becomes a sibling `.html` page,
 * everything else (the README) goes to GitHub — it does not exist in
 * `_site/`. */
/** `marked` does not emit heading ids, so in-page anchors (`#some-heading`)
 * would dead-link. Inject GitHub-style slugs: lowercase, drop anything that
 * is not alphanumeric/space/hyphen, spaces to hyphens (consecutive spaces
 * survive as consecutive hyphens, which is what GitHub does with an em dash
 * — and what the docs' own cross-links assume). */
function addHeadingIds(html) {
  return html.replace(
    /<(h[23])>([\s\S]*?)<\/\1>/g,
    (whole, tag, inner) => {
      const slug = inner
        .replace(/<[^>]+>/g, "")
        .toLowerCase()
        .replace(/[^a-z0-9 -]/g, "")
        .trim()
        .replace(/ /g, "-");
      return slug ? `<${tag} id="${slug}">${inner}</${tag}>` : whole;
    },
  );
}

function rewriteLinks(md) {
  let text = md
    .replace(/\]\(\.\.\/README\.md/g, `](${REPO_BLOB}/README.md`)
    .replace(/\]\(README\.md/g, `](${REPO_BLOB}/README.md`);
  for (const doc of DOCS) {
    if (!doc.src.startsWith("docs/")) continue;
    // `](connecting.md#anchor)` -> `](connecting.html#anchor)`
    text = text.replaceAll(`](${doc.src.slice("docs/".length)}`, `](${doc.slug}.html`);
  }
  return text;
}

async function main() {
  await rm(out, { recursive: true, force: true });
  await mkdir(join(out, "docs"), { recursive: true });
  await mkdir(join(out, "assets"), { recursive: true });

  // Landing + shared chrome.
  await cp(join(siteDir, "index.html"), join(out, "index.html"));
  await cp(join(siteDir, "styles.css"), join(out, "styles.css"));
  await cp(join(siteDir, "theme.js"), join(out, "theme.js"));

  // The `curl | sh` installer, served from /pg_lens/install.sh. A build-time
  // copy of scripts/install.sh — never a second maintained copy.
  await cp(join(repo, "scripts", "install.sh"), join(out, "install.sh"));

  // Images referenced by the landing page.
  for (const img of ["demo.gif", "web-dashboard.png"]) {
    await cp(join(repo, "docs", img), join(out, "assets", img));
  }

  // Rendered markdown.
  for (const doc of DOCS) {
    const md = rewriteLinks(await readFile(join(repo, doc.src), "utf8"));
    const body = addHeadingIds(marked.parse(md, { async: false, gfm: true }));
    await writeFile(
      join(out, "docs", `${doc.slug}.html`),
      page({ title: doc.title, body, depth: 1 }),
    );
    console.log(`site: rendered ${doc.src} → docs/${doc.slug}.html`);
  }

  // The Web Lens demo build (npm run build:demo in the frontend). `demo.html`
  // becomes the directory index so the demo lives at /pg_lens/demo/.
  const distDemo = join(repo, "crates", "pg_lens_web", "frontend", "dist-demo");
  if (!existsSync(distDemo)) {
    throw new Error(
      `site: ${distDemo} is missing — run \`npm run build:demo\` in crates/pg_lens_web/frontend first`,
    );
  }
  await cp(distDemo, join(out, "demo"), { recursive: true });
  await cp(join(out, "demo", "demo.html"), join(out, "demo", "index.html"));
  await rm(join(out, "demo", "demo.html"));

  const listing = await readdir(out);
  console.log(`site: assembled _site/ → ${listing.sort().join(", ")}`);
}

await main();
