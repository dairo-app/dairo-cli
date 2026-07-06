#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const codeArg = process.argv.find((arg) => arg.startsWith("--code="));
const code = codeArg?.slice("--code=".length) || process.env.GITHUB_APP_MANIFEST_CODE;

if (!code) {
  console.error("Usage: node scripts/configure-release-github-app.mjs --code=<manifest-code>");
  process.exit(1);
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    stdio: options.stdio ?? "pipe",
    encoding: options.encoding ?? "utf8",
    input: options.input,
  });

  if (result.status !== 0) {
    const stderr = result.stderr ? `\n${result.stderr}` : "";
    throw new Error(`${command} ${args.join(" ")} failed${stderr}`);
  }

  return result.stdout;
}

const app = JSON.parse(
  run("gh", [
    "api",
    "--method",
    "POST",
    `/app-manifests/${code}/conversions`,
  ]),
);

if (!app.id || !app.pem) {
  throw new Error("GitHub did not return both app id and private key.");
}

const keyDir = mkdtempSync(join(tmpdir(), "dairo-release-app-"));
const keyPath = join(keyDir, "private-key.pem");

try {
  writeFileSync(keyPath, app.pem, { mode: 0o600 });
  run("gh", [
    "variable",
    "set",
    "RELEASE_APP_ID",
    "--repo",
    "dairo-app/dairo-cli",
    "--body",
    String(app.id),
  ]);
  run("gh", [
    "secret",
    "set",
    "RELEASE_APP_PRIVATE_KEY",
    "--repo",
    "dairo-app/dairo-cli",
    "--body-file",
    keyPath,
  ]);
} finally {
  rmSync(keyDir, { recursive: true, force: true });
}

console.log(`Configured dairo-bot app id ${app.id} for dairo-app/dairo-cli.`);
console.log("Install the app on dairo-app/dairo-cli before running the release workflow.");
