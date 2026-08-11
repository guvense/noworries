#!/usr/bin/env node
// Thin wrapper that execs the downloaded native binary with the same args.
"use strict";

const path = require("path");
const fs = require("fs");
const { spawnSync } = require("child_process");

const binName = process.platform === "win32" ? "noworries.exe" : "noworries";
const bin = path.join(__dirname, "..", binName);

if (!fs.existsSync(bin)) {
  console.error(
    "noworries: native binary not found. Reinstall the package, or build from source " +
      "(cargo install --git https://github.com/guvense/noworries)."
  );
  process.exit(1);
}

const res = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
process.exit(res.status === null ? 1 : res.status);
