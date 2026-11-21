// The one place the page learns what the current release is called. `pnpm check:version`
// fails when this drifts from the root package.json or from the README's install lines, so a
// release cannot ship with a stale page.
export const release = {
  version: "0.2.0",
  repo: "https://github.com/namdang-fdp/why",
  latest: "https://github.com/namdang-fdp/why/releases/latest",
  appImage: "WHY_0.2.0_amd64.AppImage",
  deb: "WHY_0.2.0_amd64.deb",
  sums: "SHA256SUMS",
  minGit: "2.45.1",
  license: "MIT OR Apache-2.0",
  site: "https://why.dorriss.com",
} as const;
