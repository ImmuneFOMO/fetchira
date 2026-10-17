#!/usr/bin/env node
// `npx fetchira`: install the release binary into ~/.local/bin on first run, then hand off to it.
const { spawnSync } = require("child_process");
const { existsSync } = require("fs");
const { homedir } = require("os");
const path = require("path");

const bin = path.join(process.env.BIN_DST || path.join(homedir(), ".local", "bin"), "fetchira");

function run(cmd, args, stdio) {
  const { status, error } = spawnSync(cmd, args, { stdio });
  if (error) console.error(error.message);
  return status ?? 1;
}

if (!existsSync(bin)) {
  if (process.platform !== "darwin" && process.platform !== "linux") {
    console.error(
      `fetchira ships macOS and Linux binaries only (this is ${process.platform}); on Windows use WSL.\n` +
        "Homebrew and curl | sh: https://github.com/ImmuneFOMO/fetchira#install",
    );
    process.exit(1);
  }
  // Closed stdin keeps install.sh from launching `fetchira setup` itself; the hand-off below opens the dashboard.
  const status = run("sh", [path.join(__dirname, "..", "install.sh")], ["ignore", "inherit", "inherit"]);
  if (status !== 0) process.exit(status);
}
process.exit(run(bin, process.argv.slice(2), "inherit"));
