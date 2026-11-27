// Builds the npm packages from the release archives.
//
//   node npm/build.mjs <version> <archives-dir> <out-dir> [--partial]
//
// <archives-dir> holds marklark-<version>-<target>.{tar.gz,zip} as the `binaries` job makes them.
// Writes <out-dir>/<platform>/ (one package per platform: @verhas/marklark-<platform>) and
// <out-dir>/marklark/ (the main package, depending on all of them). --partial skips missing
// platforms (for local testing); a release needs all of them.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.dirname(here);
const [version, archives, out] = process.argv.slice(2);
const partial = process.argv.includes("--partial");
if (!version || !archives || !out) {
  console.error("usage: node npm/build.mjs <version> <archives-dir> <out-dir> [--partial]");
  process.exit(2);
}
const platforms = JSON.parse(fs.readFileSync(path.join(here, "platforms.json"), "utf8"));
const common = {
  version,
  description: "Keeps Markdown documents up to date: humans write intent, agents write code, marklark runs it.",
  license: "MIT OR Apache-2.0",
  homepage: "https://github.com/verhas/marklark",
  repository: { type: "git", url: "git+https://github.com/verhas/marklark.git" },
};
const licenses = ["LICENSE", "LICENSE-MIT", "LICENSE-APACHE"];

fs.rmSync(out, { recursive: true, force: true });
const built = [];
for (const p of platforms) {
  const name = `marklark-${version}-${p.target}`;
  const archive = path.join(archives, `${name}.${p.archive}`);
  if (!fs.existsSync(archive)) {
    if (partial) { console.warn(`skipping ${p.pkg}: no ${archive}`); continue; }
    console.error(`missing ${archive}`);
    process.exit(1);
  }
  const tmp = fs.mkdtempSync(path.join(out + "-tmp-"));
  if (p.archive === "zip") execFileSync("unzip", ["-q", archive, "-d", tmp]);
  else execFileSync("tar", ["xzf", archive, "-C", tmp]);
  const dir = path.join(out, p.pkg);
  fs.mkdirSync(path.join(dir, "bin"), { recursive: true });
  const exe = path.join(dir, "bin", p.exe);
  fs.copyFileSync(path.join(tmp, name, p.exe), exe);
  fs.chmodSync(exe, 0o755);
  fs.rmSync(tmp, { recursive: true, force: true });
  for (const l of licenses) fs.copyFileSync(path.join(root, l), path.join(dir, l));
  fs.writeFileSync(path.join(dir, "README.md"),
    `# @verhas/marklark-${p.pkg}\n\nThe marklark binary for ${p.os} ${p.cpu}. Install \`marklark\` instead; it picks this package for you.\n`);
  fs.writeFileSync(path.join(dir, "package.json"), JSON.stringify({
    name: `@verhas/marklark-${p.pkg}`, ...common, os: [p.os], cpu: [p.cpu],
    files: ["bin", ...licenses], preferUnplugged: true,
  }, null, 2) + "\n");
  built.push(p.pkg);
}

const main = path.join(out, "marklark");
fs.cpSync(path.join(here, "marklark"), main, { recursive: true });
for (const l of licenses) fs.copyFileSync(path.join(root, l), path.join(main, l));
fs.copyFileSync(path.join(root, "README.md"), path.join(main, "README.md"));
fs.writeFileSync(path.join(main, "package.json"), JSON.stringify({
  name: "marklark", ...common,
  bin: { marklark: "bin/marklark.js" },
  files: ["bin", ...licenses],
  engines: { node: ">=16" },
  optionalDependencies: Object.fromEntries(built.map((p) => [`@verhas/marklark-${p}`, version])),
}, null, 2) + "\n");
console.log(`built marklark ${version}: ${built.join(", ")}`);
