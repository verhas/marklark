#!/usr/bin/env node
// Runs the marklark binary from the platform package npm installed alongside this one
// (one of the optionalDependencies; npm picks it by `os` and `cpu`).
"use strict";
const { spawnSync } = require("child_process");
const path = require("path");

const pkg = `@verhas/marklark-${process.platform}-${process.arch}`;
const exe = process.platform === "win32" ? "marklark.exe" : "marklark";
let binary;
try {
  binary = path.join(path.dirname(require.resolve(`${pkg}/package.json`)), "bin", exe);
} catch {
  console.error(
    `marklark: no binary for ${process.platform}-${process.arch} (package ${pkg} is not installed).\n` +
      "Supported: linux-x64, linux-arm64, darwin-x64, darwin-arm64, win32-x64.\n" +
      "If you installed with --no-optional or --omit=optional, reinstall without it.\n" +
      "Other ways to install: cargo install marklark, pip install marklark."
  );
  process.exit(2);
}
const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`marklark: cannot run ${binary}: ${result.error.message}`);
  process.exit(2);
}
if (result.signal) process.kill(process.pid, result.signal);
process.exit(result.status ?? 1);
