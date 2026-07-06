#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { readFileSync } from "node:fs";

const repo = "dairo-app/dairo-cli";
const org = "dairo-app";
const port = Number(process.env.PORT || 49217);
const callbackUrl = `http://127.0.0.1:${port}/callback`;
const manifestPath = new URL("../ops/github-apps/dairo-bot-manifest.json", import.meta.url);
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));

manifest.redirect_url = callbackUrl;
manifest.callback_urls = [callbackUrl];

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

function configureApp(code) {
  const app = JSON.parse(
    run("gh", ["api", "--method", "POST", `/app-manifests/${code}/conversions`]),
  );

  if (!app.id || !app.pem) {
    throw new Error("GitHub did not return both app id and private key.");
  }

  run("gh", ["variable", "set", "RELEASE_APP_ID", "--repo", repo, "--body", String(app.id)]);
  run("gh", ["secret", "set", "RELEASE_APP_PRIVATE_KEY", "--repo", repo], { input: app.pem });

  return app;
}

const server = createServer((req, res) => {
  const url = new URL(req.url ?? "/", callbackUrl);

  if (url.pathname === "/") {
    res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    res.end(`<!doctype html>
<html lang="en">
  <head><meta charset="utf-8"><title>Create dairo-bot</title></head>
  <body>
    <form action="https://github.com/organizations/${org}/settings/apps/new" method="post">
      <input name="manifest" type="hidden" value="${JSON.stringify(manifest).replaceAll("&", "&amp;").replaceAll('"', "&quot;")}">
      <button type="submit">Create dairo-bot</button>
    </form>
    <script>document.querySelector("form").submit();</script>
  </body>
</html>`);
    return;
  }

  if (url.pathname !== "/callback") {
    res.writeHead(404);
    res.end("Not found");
    return;
  }

  const code = url.searchParams.get("code");
  if (!code) {
    res.writeHead(400);
    res.end("Missing code.");
    return;
  }

  try {
    const app = configureApp(code);
    res.writeHead(200, { "content-type": "text/plain; charset=utf-8" });
    res.end(`Configured dairo-bot app id ${app.id} for ${repo}.\nNow install it on ${repo}, selected repositories only.\n`);
    console.log(`Configured dairo-bot app id ${app.id} for ${repo}.`);
    console.log(`Install URL: ${app.html_url}/installations/new`);
    setImmediate(() => server.close());
  } catch (error) {
    console.error(error);
    res.writeHead(500, { "content-type": "text/plain; charset=utf-8" });
    res.end(String(error));
  }
});

server.listen(port, "127.0.0.1", () => {
  const url = `http://127.0.0.1:${port}/`;
  console.log(`Opening ${url}`);
  spawnSync("open", [url], { stdio: "ignore" });
});
