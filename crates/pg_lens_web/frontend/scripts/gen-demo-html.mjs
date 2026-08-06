// Generates `demo.html` — the GitHub Pages demo entry — from the production
// `index.html`, at build time.
//
// Why generate instead of committing a second HTML file: index.html is the
// real app shell (icon sprite, every panel, the token overlay). A committed
// copy would drift the moment anyone touches a panel. This script does two
// surgical substitutions and nothing else:
//
//   1. swaps the module entry `/src/main.ts` → `/src/demo.ts` (the shim that
//      installs the fixture-replaying stubs and *then* imports main.ts), and
//   2. injects the persistent "DEMO — canned data" badge into the topbar,
//      plus its (small, token-based) styles.
//
// `demo.html` is gitignored and only ever consumed by `vite build --mode demo`
// (see vite.config.ts). The production build's entry, config and output are
// untouched.

import { readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

const SPACER = '<div class="topbar-spacer"></div>';

const BADGE = `<a
          class="demo-badge"
          href="/pg_lens/"
          title="This dashboard is replaying a recorded snapshot sequence — it is not connected to any PostgreSQL server"
        >
          <span class="demo-badge-dot" aria-hidden="true"></span>
          <strong>DEMO</strong>
          <span class="demo-badge-text">canned data, no live database</span>
        </a>
        <a class="demo-link" href="/pg_lens/">About</a>
        <a class="demo-link" href="https://github.com/dog-hero/pg_lens">GitHub</a>
        ${SPACER}`;

// Uses only the app's own design tokens so the badge stays correct in both
// themes without duplicating a palette.
const STYLE = `<style>
      .demo-badge {
        display: inline-flex;
        align-items: center;
        gap: 0.4rem;
        margin-left: 0.75rem;
        padding: 0.15rem 0.55rem;
        border: 1px solid var(--warn);
        border-radius: 999px;
        background: var(--warn-bg);
        color: var(--warn);
        font-size: 0.75rem;
        font-weight: 600;
        letter-spacing: 0.02em;
        text-decoration: none;
        white-space: nowrap;
      }
      .demo-badge-dot {
        width: 0.5rem;
        height: 0.5rem;
        border-radius: 50%;
        background: var(--warn);
      }
      .demo-badge-text { font-weight: 400; }
      .demo-link {
        margin-left: 0.6rem;
        color: var(--fg-dim);
        font-size: 0.78rem;
        text-decoration: none;
      }
      .demo-link:hover { color: var(--accent); text-decoration: underline; }
      @media (max-width: 900px) {
        .demo-badge-text, .demo-link { display: none; }
      }
    </style>
  </head>`;

const html = await readFile(join(root, "index.html"), "utf8");

function replaceOnce(source, needle, replacement, what) {
  const first = source.indexOf(needle);
  if (first === -1) throw new Error(`gen-demo-html: could not find ${what} in index.html`);
  if (source.indexOf(needle, first + 1) !== -1) {
    throw new Error(`gen-demo-html: ${what} is not unique in index.html`);
  }
  return source.slice(0, first) + replacement + source.slice(first + needle.length);
}

let out = replaceOnce(html, '<title>pg_lens</title>', '<title>pg_lens — live demo</title>', "the title");
out = replaceOnce(out, "</head>", STYLE, "the </head> tag");
out = replaceOnce(out, SPACER, BADGE, "the topbar spacer");
out = replaceOnce(out, '"/src/main.ts"', '"/src/demo.ts"', "the module entry");

await writeFile(join(root, "demo.html"), out);
console.log("gen-demo-html: wrote demo.html");
