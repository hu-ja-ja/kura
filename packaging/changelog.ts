import { readFileSync, writeFileSync } from "node:fs";

const VERSION_HEADER = /^## \[?v?(\d+\.\d+\.\d+)\]?(?:\(|\s|$)/;

export function extractVersionSection(
  changelog: string,
  version: string,
): string | undefined {
  const lines = changelog.split(/\r?\n/);
  const start = lines.findIndex((line) => VERSION_HEADER.exec(line)?.[1] === version);
  if (start < 0) return undefined;

  let end = lines.length;
  for (let index = start + 1; index < lines.length; index += 1) {
    if (VERSION_HEADER.test(lines[index])) {
      end = index;
      break;
    }
  }

  const section = lines.slice(start + 1, end).join("\n").trim();
  return section || undefined;
}

function main(args: string[]): void {
  const check = args.includes("--check");
  const [rawVersion, output] = args.filter((arg) => arg !== "--check");
  if (!rawVersion) {
    console.error("usage: node packaging/changelog.ts <version> [output] [--check]");
    process.exit(1);
  }

  const version = rawVersion.replace(/^v/, "");
  const section = extractVersionSection(readFileSync("CHANGELOG.md", "utf8"), version);
  if (!section) {
    console.error(`CHANGELOG.md has no section for ${version}`);
    process.exit(1);
  }

  if (check) return;
  if (output) {
    writeFileSync(output, `${section}\n`);
  } else {
    console.log(section);
  }
}

if (import.meta.main) {
  main(process.argv.slice(2));
}
