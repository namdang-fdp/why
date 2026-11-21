import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { createMarkdownProcessor } from "@astrojs/markdown-remark";
import { release } from "./release";

// The changelog page is rendered at build time from the app repo's own CHANGELOG.md, so there is
// no second copy to drift. The build runs from site/, the file lives one level up.
export interface ChangelogEntry {
  version: string;
  date: string | null;
  html: string;
}

const HEADING = /^## \[([^\]]+)\](?: - (\d{4}-\d{2}-\d{2}))?[ \t]*$/;
const LINK_DEFINITION = /^\[[^\]]+\]: \S+$/;

export async function loadChangelog(): Promise<ChangelogEntry[]> {
  const source = await readFile(resolve(process.cwd(), "../CHANGELOG.md"), "utf8");
  // No syntax highlighting: it would write inline styles, which the page's CSP forbids.
  const markdown = await createMarkdownProcessor({ syntaxHighlight: false });

  const sections: { version: string; date: string | null; lines: string[] }[] = [];
  for (const line of source.split("\n")) {
    const match = HEADING.exec(line);
    if (match) {
      sections.push({ version: match[1], date: match[2] ?? null, lines: [] });
    } else if (!LINK_DEFINITION.test(line)) {
      sections.at(-1)?.lines.push(line); // text before the first version heading is dropped
    }
  }

  const entries: ChangelogEntry[] = [];
  for (const section of sections) {
    const body = section.lines.join("\n").trim();
    if (!body) continue; // an empty "Unreleased" section shows nothing
    const { code } = await markdown.render(body);
    entries.push({
      version: section.version,
      date: section.date,
      html: code
        // Headings repeat across versions ("Added"), so their generated ids would be duplicates.
        .replace(/<h3 id="[^"]*"/g, "<h3")
        // Links inside the repo (SECURITY.md, docs/...) must point at GitHub from here.
        .replace(/href="(?!https?:|#|\/|mailto:)([^"]+)"/g, `href="${release.repo}/blob/main/$1"`),
    });
  }
  if (!entries.some((entry) => /^\d/.test(entry.version))) {
    throw new Error(
      "CHANGELOG.md has no released version section; the changelog page would be empty",
    );
  }
  return entries;
}
