import { release } from "./release";

// Every outbound link on the page, derived from the repo URL in release.ts.
// `pnpm check:links` requests each of them.
const blob = `${release.repo}/blob/main`;

export const links = {
  repo: release.repo,
  latest: release.latest,
  sums: `${release.repo}/releases/latest/download/${release.sums}`,
  changelog: `${blob}/CHANGELOG.md`,
  security: `${blob}/SECURITY.md`,
  licenseMit: `${blob}/LICENSE-MIT`,
  licenseApache: `${blob}/LICENSE-APACHE`,
  issues: `${release.repo}/issues`,
  research: `${blob}/why-brain/05%20-%20Research.md`,
} as const;
