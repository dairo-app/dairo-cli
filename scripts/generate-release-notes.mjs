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

function renderList(items, empty = "No notable changes.") {
  const list = normalizeList(items);
  if (!list.length) return `- ${empty}\n`;
  return list.map((item) => `- ${item}`).join("\n") + "\n";
}

function renderNotes(data) {
  const notes = {
    highlights: normalizeList(data?.highlights),
    added: normalizeList(data?.added),
    changed: normalizeList(data?.changed),
    fixed: normalizeList(data?.fixed),
    security: normalizeList(data?.security),
  };

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
    "Verify:",
    "",
    "```sh",
    "dairo --version",
    "dairo doctor",
    "```",
    "",
    "## Highlights",
    "",
    renderList(notes.highlights).trimEnd(),
    "",
    "## What Changed",
    "",
    "### Added",
    "",
    renderList(notes.added).trimEnd(),
    "",
    "### Changed",
    "",
    renderList(notes.changed).trimEnd(),
    "",
    "### Fixed",
    "",
    renderList(notes.fixed).trimEnd(),
    "",
    "### Security",
    "",
    renderList(notes.security, "No security changes in this release.").trimEnd(),
    "",
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
    "Use only the provided git data. Do not invent features.",
    "Prefer user-facing wording over internal implementation details.",
    "Keep each bullet concise.",
    "Mention breaking changes under changed.",
    "Mention auth, secret handling, TLS, permissions, or dependency security fixes under security.",
    "",
    previous ? `Previous tag: ${previous}` : "Previous tag: none; this is the first release.",
    "",
    "Commit log:",
    commitLog || "(none)",
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
