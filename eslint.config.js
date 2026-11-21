import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["**/dist", "**/node_modules", "src-tauri", "site"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    // Webview code: browser globals only, so `process`, `Buffer`, etc. are errors.
    files: ["ui/src/**/*.{ts,tsx}"],
    languageOptions: { globals: globals.browser },
    plugins: { "react-hooks": reactHooks },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // typescript-eslint disables `no-undef`, so forbid Node globals explicitly (tsc also rejects them).
      "no-restricted-globals": ["error", "process", "Buffer", "__dirname", "__filename", "require"],
    },
  },
  {
    // Tooling code that runs in Node.
    files: ["ui/*.ts", "scripts/**/*.mjs", "eslint.config.js"],
    languageOptions: { globals: globals.node },
  },
);
