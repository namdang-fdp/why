import type { languages } from "monaco-editor";

export function detectLanguage(
  path: string,
  registered: readonly languages.ILanguageExtensionPoint[],
) {
  const name = path.slice(path.lastIndexOf("/") + 1).toLowerCase();
  let match = "plaintext";
  let longest = 0;

  for (const language of registered) {
    if (language.filenames?.some((filename) => filename.toLowerCase() === name)) {
      return language.id;
    }
    for (const extension of language.extensions ?? []) {
      if (extension.length > longest && name.endsWith(extension.toLowerCase())) {
        longest = extension.length;
        match = language.id;
      }
    }
  }
  return match;
}
