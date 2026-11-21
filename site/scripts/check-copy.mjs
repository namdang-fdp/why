// Guards the honesty rules in the design spec: no page may make claims the project cannot back,
// and the pages must keep saying what is early, planned or limited. Reads each built page as text.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const banned = [
  "trusted by",
  "testimonial",
  "★",
  "customers",
  "% faster",
  "used by",
  "join thousands",
  "production-ready",
  "enterprise-grade",
];

// Phrases each page must contain. Matching is on visible text, case-insensitive.
const pages = {
  "index.html": [
    "planned",
    "Linux",
    "Git 2.45.1",
    "SHA256SUMS",
    "Observed",
    "Declared",
    "Unknown",
    "MIT OR Apache-2.0",
    "early",
    "not shipped",
    "removes only WHY's entries",
    "/hooks",
    "every repo and worktree you add",
  ],
  "about/index.html": [
    "maintained by one person",
    "local-first",
    "planned",
    "not the first",
    "MIT OR Apache-2.0",
  ],
  "changelog/index.html": ["0.2.1", "0.2.0", "0.1.0", "CHANGELOG.md", "Keep a Changelog"],
};

const errors = [];
for (const [file, required] of Object.entries(pages)) {
  const html = readFileSync(fileURLToPath(new URL(`../dist/${file}`, import.meta.url)), "utf8");
  // Visible text only: drop scripts, styles and tags, then collapse whitespace.
  const text = html
    .replace(/<(script|style)[\s\S]*?<\/\1>/gi, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/&#(?:39|x27);/g, "'")
    .replace(/&quot;/g, '"')
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ");
  const lower = text.toLowerCase();
  const found = [
    ...banned.filter((p) => lower.includes(p.toLowerCase())).map((p) => `banned phrase: "${p}"`),
    ...required
      .filter((p) => !lower.includes(p.toLowerCase()))
      .map((p) => `missing phrase: "${p}"`),
    // Formatters can drop the space between a word and an inline element ("The<a>link</a>"),
    // which renders as one glued word. Fail if any appears.
    ...[
      ...(html.match(/[A-Za-z,.]<(?:a|code|em|strong|span)[ >][^>]*>[^<]{0,20}/g) ?? []),
      ...(html.match(/<\/(?:a|code|em|strong|span)>[A-Za-z]\w{0,20}/g) ?? []),
    ].map((piece) => `word glued to an inline element: ${piece}`),
  ];
  errors.push(...found.map((message) => `[${file}] ${message}`));
}

if (errors.length > 0) {
  console.error(errors.join("\n"));
  process.exit(1);
}
console.log(
  `copy ok on ${Object.keys(pages).length} pages: no banned phrases, required phrases present, no glued words`,
);
