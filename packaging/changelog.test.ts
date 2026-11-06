import assert from "node:assert/strict";
import { test } from "node:test";
import { extractVersionSection } from "./changelog.ts";

const changelog = `# Changelog

## [Unreleased]

## [0.2.0](https://github.com/hu-ja-ja/kura/releases/tag/v0.2.0) - 2026-10-30

### Added
- New thing

### Fixed
- Bug

## [0.1.0](https://github.com/hu-ja-ja/kura/releases/tag/v0.1.0) - 2026-10-29

### Added
- Old thing
`;

test("extracts a linked version section", () => {
  assert.equal(
    extractVersionSection(changelog, "0.2.0"),
    "### Added\n- New thing\n\n### Fixed\n- Bug",
  );
});

test("extracts the oldest version to the end", () => {
  assert.equal(extractVersionSection(changelog, "0.1.0"), "### Added\n- Old thing");
});

test("returns undefined for a missing version", () => {
  assert.equal(extractVersionSection(changelog, "9.9.9"), undefined);
});

test("does not match a version prefix", () => {
  assert.equal(extractVersionSection(changelog, "0.2"), undefined);
});

test("ignores prerelease versions", () => {
  const input = "## [0.2.0-rc.1] - 2026-10-30\n\n- Candidate";
  assert.equal(extractVersionSection(input, "0.2.0"), undefined);
});

test("handles CRLF", () => {
  assert.equal(
    extractVersionSection(changelog.replace(/\n/g, "\r\n"), "0.1.0"),
    "### Added\n- Old thing",
  );
});
