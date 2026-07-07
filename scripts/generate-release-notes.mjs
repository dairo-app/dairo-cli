#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";

const tag = process.env.RELEASE_TAG || "";
const out = process.env.RELEASE_NOTES_FILE || "release-notes.md";
const project = process.env.GCP_PROJECT_ID || "";
const location = process.env.GCP_LOCATION || "eu";
const model = process.env.VERTEX_MODEL || "gemini-3.5-flash";
const token = process.env.VERTEX_ACCESS_TOKEN || "";

function git(args, fallback = "") {
  try {
    return execFileSync("git", args, { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  } catch {
    return fallback;
  }
}

function cargoVersion() {
  try {
    const cargoToml = execFileSync("sed", ["-n", 's/^version = "\\(.*\\)"/\\1/p', "Cargo.toml"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
    return cargoToml.split("\n")[0] || "";
  } catch {
    return "";
  }
}

const version = process.env.RELEASE_VERSION || tag.replace(/^v/, "") || cargoVersion();
const releaseLabel = tag || `v${version}`;

function previousTag() {
  const tags = git(["tag", "--sort=-creatordate", "--merged", "HEAD"]).split("\n").filter(Boolean);
  return tags.find((candidate) => candidate !== tag) || "";
}

const previous = previousTag();
const range = previous ? `${previous}..HEAD` : "HEAD";
const commitLog = git(["log", "--pretty=format:%h %s", "--max-count=80", range]);
// Commit bodies usually carry the user-facing detail that subjects compress
// away; cap the total so a huge range cannot blow the prompt.
const commitBodies = git(["log", "--pretty=format:%h %s%n%b", "--max-count=40", range]).slice(0, 12000);
const diffStat = git(["diff", "--stat", range]);
const nameStatus = git(["diff", "--name-status", range]);

const releaseSchema = {
  type: "OBJECT",
  properties: {
    highlights: {
      type: "ARRAY",
      items: { type: "STRING" },
    },
    added: {
      type: "ARRAY",
      items: { type: "STRING" },
    },
    changed: {
      type: "ARRAY",
      items: { type: "STRING" },
    },
    fixed: {
      type: "ARRAY",
      items: { type: "STRING" },
    },
    security: {
      type: "ARRAY",
      items: { type: "STRING" },
    },
  },
  required: ["highlights", "added", "changed", "fixed", "security"],
};

function normalizeList(value) {
  if (!Array.isArray(value)) return [];
  return value
    .map((item) => String(item || "").trim())
    .filter(Boolean)
    .slice(0, 8);
}

function renderList(items) {
  return normalizeList(items)
    .map((item) => `- ${item}`)
    .join("\n");
}

function renderNotes(data) {
  const notes = {
    highlights: normalizeList(data?.highlights),
    added: normalizeList(data?.added),
    changed: normalizeList(data?.changed),
    fixed: normalizeList(data?.fixed),
    security: normalizeList(data?.security),
  };

  const sections = [
    ["Added", notes.added],
    ["Changed", notes.changed],
    ["Fixed", notes.fixed],
    ["Security", notes.security],
  ].filter(([, items]) => items.length);

  const changeBlocks = sections.length
    ? sections.flatMap(([title, items]) => [`### ${title}`, "", renderList(items), ""])
    : ["Maintenance release: internal build and release-pipeline work only, no user-facing changes.", ""];

  const highlightBlock = notes.highlights.length
    ? renderList(notes.highlights)
    : "- Maintenance release with no user-facing changes.";

  return [
    `# Dairo CLI ${releaseLabel}`,
    "",
    "Official Dairo CLI release for macOS, Linux, and Windows.",
    "",
    "## Install",
    "",
    "macOS and Linux:",
    "",
    "```sh",
    "curl -fsSL https://dairo.app/install.sh | sh",
    "```",
    "",
    "Windows PowerShell:",
    "",
    "```powershell",
    "irm https://dairo.app/install.ps1 | iex",
    "```",
    "",
    "Homebrew:",
    "",
    "```sh",
    "brew install dairo-app/tap/dairo",
    "```",
    "",
    "Already installed? `dairo update` (or `brew upgrade dairo`). Verify with `dairo --version` and `dairo doctor`.",
    "",
    "## Highlights",
    "",
    highlightBlock,
    "",
    "## What Changed",
    "",
    ...changeBlocks,
    "## Compatibility",
    "",
    "- Supports macOS, Linux, and Windows on the published architectures.",
    `- CLI version: \`${releaseLabel}\``,
    "- API compatibility: current Dairo API.",
    "",
    "## Support",
    "",
    "- Docs: https://docs.dairo.app",
    "- Issues: https://github.com/dairo-app/dairo-cli/issues",
    "",
    "Built and released by the Dairo team.",
    "",
  ].join("\n");
}

async function generateWithVertex() {
  if (!project) {
    throw new Error("GCP_PROJECT_ID is required.");
  }
  if (!token) {
    throw new Error("VERTEX_ACCESS_TOKEN is required.");
  }

  const host = ["eu", "global", "us"].includes(location)
    ? "aiplatform.googleapis.com"
    : `${location}-aiplatform.googleapis.com`;
  const endpoint = `https://${host}/v1/projects/${project}/locations/${location}/publishers/google/models/${model}:generateContent`;
  const prompt = [
    `Summarize Dairo CLI ${releaseLabel} changes as JSON for release notes.`,
    "The audience is end users of the `dairo` command-line tool, not maintainers of this repository.",
    "Use only the provided git data. Do not invent features.",
    "Only include changes a CLI user would notice: new or changed commands and flags, behavior, output, installation, or self-update.",
    "Internal-only work (CI workflows, release pipeline, refactors, tests, docs tooling) must NOT appear in highlights, added, changed, or fixed. If every change in the range is internal, return empty arrays for all fields.",
    "Keep each bullet concise and written for someone running the CLI.",
    "Mention breaking changes under changed, prefixed with 'Breaking:'.",
    "Use security only for fixes that affect users: credential handling, TLS, permissions of files the CLI writes, or vulnerable dependency upgrades in the shipped binary.",
    "",
    previous ? `Previous tag: ${previous}` : "Previous tag: none; this is the first release.",
    "",
    "Commit log:",
    commitLog || "(none)",
    "",
    "Commit messages with bodies:",
    commitBodies || "(none)",
    "",
    "Diff stat:",
    diffStat || "(none)",
    "",
    "Changed files:",
    nameStatus || "(none)",
  ].join("\n");

  const res = await fetch(endpoint, {
    method: "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      contents: [{ role: "user", parts: [{ text: prompt }] }],
      generationConfig: {
        temperature: 0.2,
        maxOutputTokens: 1400,
        responseMimeType: "application/json",
        responseSchema: releaseSchema,
      },
    }),
  });

  if (!res.ok) {
    const body = await res.text();
    throw new Error(`Vertex AI returned ${res.status}: ${body.slice(0, 1000)}`);
  }

  const json = await res.json();
  const text = json.candidates?.[0]?.content?.parts?.map((part) => part.text || "").join("\n");
  if (!text) {
    throw new Error("Vertex AI returned no structured release-note payload.");
  }
  return JSON.parse(text);
}

const generated = await generateWithVertex();
writeFileSync(out, renderNotes(generated));
console.log(`Generated structured release notes with Vertex AI ${model}.`);
