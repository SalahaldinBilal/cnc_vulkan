// Run with: npm run version:bump <version>  (e.g. 0.2.0)
// Sets the version in Cargo.toml and Cargo.lock, and turns CHANGELOG.md's "Unreleased" section into a dated section
// for that version, which the release workflow uses as the release notes.

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const newVersion = process.argv[2];

if (!newVersion || !/^\d+\.\d+\.\d+$/.test(newVersion)) {
  console.error("Usage: npm run version:bump <version>  (e.g. 0.2.0)");
  process.exit(1);
}

function pathOf(relativePath: string): string {
  return fileURLToPath(new URL(`../${relativePath}`, import.meta.url));
}

function replaceOrThrow(relativePath: string, pattern: RegExp, replacement: string) {
  const contents = readFileSync(pathOf(relativePath), "utf-8");
  if (!pattern.test(contents)) {
    throw new Error(`Could not find a version matching ${pattern} in ${relativePath}`);
  }
  writeFileSync(pathOf(relativePath), contents.replace(pattern, replacement));
  console.log(`Updated ${relativePath}`);
}

function cutChangelog() {
  const relativePath = "CHANGELOG.md";
  const contents = readFileSync(pathOf(relativePath), "utf-8");

  const unreleasedHeading = "## [Unreleased]";
  const headingStart = contents.indexOf(unreleasedHeading);
  if (headingStart === -1) {
    throw new Error(`Could not find "${unreleasedHeading}" in ${relativePath}`);
  }

  const sectionStart = headingStart + unreleasedHeading.length;
  const nextHeadingIndex = contents.indexOf("\n## [", sectionStart);
  const sectionEnd = nextHeadingIndex === -1 ? contents.length : nextHeadingIndex + 1;
  const sectionBody = contents.slice(sectionStart, sectionEnd).trim();

  if (!sectionBody) {
    console.warn(`Warning: the "${unreleasedHeading}" section in ${relativePath} is empty, so this release has no notes.`);
  }

  const date = new Date().toISOString().slice(0, 10);
  const releasedHeading = `## [${newVersion}] - ${date}`;
  let replacement = sectionBody
    ? `${unreleasedHeading}\n\n${releasedHeading}\n\n${sectionBody}\n`
    : `${unreleasedHeading}\n\n${releasedHeading}\n`;
  if (sectionEnd < contents.length) {
    replacement += "\n";
  }

  writeFileSync(pathOf(relativePath), contents.slice(0, headingStart) + replacement + contents.slice(sectionEnd));
  console.log(`Updated ${relativePath}`);
}

if (readFileSync(pathOf("CHANGELOG.md"), "utf-8").includes(`## [${newVersion}]`)) {
  console.error(`CHANGELOG.md already has a section for ${newVersion}, so it was already bumped.`);
  process.exit(1);
}

replaceOrThrow("Cargo.toml", /(?<=^\[workspace\.package\]\r?\nversion = ")[^"]*/m, newVersion);
replaceOrThrow("Cargo.lock", /(?<=name = "d3d8"\r?\nversion = ")[^"]*/, newVersion);
replaceOrThrow("Cargo.lock", /(?<=name = "bigtool"\r?\nversion = ")[^"]*/, newVersion);
cutChangelog();

console.log(`\nBumped to ${newVersion}. Tag it as v${newVersion}.`);
