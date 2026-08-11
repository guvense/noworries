#!/usr/bin/env node
// postinstall: download the prebuilt noworries binary for this platform from
// the matching GitHub release and place it next to this file.
"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const https = require("https");
const { execFileSync } = require("child_process");

const REPO = process.env.NOWORRIES_REPO || "guvense/noworries";
const version = require("./package.json").version;

function target() {
  const p = process.platform;
  const a = process.arch;
  if (p === "darwin" && a === "arm64") return "aarch64-apple-darwin";
  if (p === "darwin" && a === "x64") return "x86_64-apple-darwin";
  if (p === "linux" && a === "x64") return "x86_64-unknown-linux-gnu";
  return null;
}

function download(url, dest, redirects = 0) {
  return new Promise((resolve, reject) => {
    if (redirects > 5) return reject(new Error("too many redirects"));
    https
      .get(url, { headers: { "User-Agent": "noworries-npm" } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          res.resume();
          return resolve(download(res.headers.location, dest, redirects + 1));
        }
        if (res.statusCode !== 200) {
          res.resume();
          return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
        }
        const file = fs.createWriteStream(dest);
        res.pipe(file);
        file.on("finish", () => file.close(() => resolve()));
        file.on("error", reject);
      })
      .on("error", reject);
  });
}

async function main() {
  const t = target();
  if (!t) {
    console.error(
      `noworries: no prebuilt binary for ${process.platform}/${process.arch}. ` +
        `Install with: cargo install --git https://github.com/${REPO}`
    );
    process.exit(1);
  }

  const asset = `noworries-${t}.tar.gz`;
  const url = `https://github.com/${REPO}/releases/download/v${version}/${asset}`;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "noworries-"));
  const tarball = path.join(tmp, asset);

  console.log(`noworries: downloading ${asset} (v${version})…`);
  try {
    await download(url, tarball);
    // tar is available on macOS and Linux (and modern Windows).
    execFileSync("tar", ["-xzf", tarball, "-C", __dirname], { stdio: "inherit" });
    const bin = path.join(__dirname, "noworries");
    fs.chmodSync(bin, 0o755);
    console.log("noworries: installed. Run `noworries install-command` to add the /noworries slash command.");
  } catch (e) {
    console.error(`noworries: install failed: ${e.message}`);
    console.error(`  You can build from source: cargo install --git https://github.com/${REPO}`);
    process.exit(1);
  } finally {
    try {
      fs.rmSync(tmp, { recursive: true, force: true });
    } catch (_) {}
  }
}

main();
