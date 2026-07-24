#!/usr/bin/env node
// Standalone no-internal-JSON scanner for Node repositories.
// It has no npm dependency and emits Markdown plus HBP evidence only.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const patterns = [
  ["serialization-library", "serde_json"], ["serialization-library", "orjson"],
  ["serialization-call", "json.dumps"], ["serialization-call", "json.loads"],
  ["serialization-call", "JSON.parse"], ["serialization-call", "JSON.stringify"],
  ["protocol", "JSON-RPC"], ["protocol", "json-rpc"],
];
const ignored = new Set([".git", "target", "node_modules", "vendor", ".venv", ".simplicio"]);
const jsonExtensions = new Set([".json", ".jsonl", ".ndjson"]);
const datePattern = /^\d{4}-\d{2}-\d{2}$/;

function compareText(left, right) {
  const leftCodePoints = Array.from(left);
  const rightCodePoints = Array.from(right);
  const length = Math.min(leftCodePoints.length, rightCodePoints.length);
  for (let index = 0; index < length; index += 1) {
    const leftCodePoint = leftCodePoints[index].codePointAt(0);
    const rightCodePoint = rightCodePoints[index].codePointAt(0);
    if (leftCodePoint !== rightCodePoint) return leftCodePoint < rightCodePoint ? -1 : 1;
  }
  return leftCodePoints.length - rightCodePoints.length;
}

function stripComment(line) {
  let quote = null;
  let escaped = false;
  for (let index = 0; index < line.length; index += 1) {
    const character = line[index];
    if (quote === '"') {
      if (escaped) escaped = false;
      else if (character === "\\") escaped = true;
      else if (character === '"') quote = null;
    } else if (quote === "'") {
      if (character === "'") quote = null;
    } else if (character === '"' || character === "'") {
      quote = character;
    } else if (character === "#") {
      return line.slice(0, index);
    }
  }
  return line;
}

function parseTomlValue(value) {
  if (value.startsWith('"') && value.endsWith('"')) return JSON.parse(value);
  if (value.startsWith("'") && value.endsWith("'")) return value.slice(1, -1);
  if (value === "[]") return [];
  if (/^[+-]?\d(?:_?\d)*$/.test(value)) return Number(value.replaceAll("_", ""));
  throw new Error(`unsupported TOML value: ${value}`);
}

function policyFromToml(text, today) {
  const policy = { exceptions: [] };
  let current = null;
  for (const raw of text.split(/\r?\n/)) {
    const line = stripComment(raw).trim();
    if (!line) continue;
    if (line === "[[exceptions]]") { current = {}; policy.exceptions.push(current); continue; }
    const split = line.indexOf("=");
    if (split < 0) throw new Error(`invalid policy line: ${line}`);
    const key = line.slice(0, split).trim();
    const value = line.slice(split + 1).trim();
    const target = current ?? policy;
    target[key] = parseTomlValue(value);
  }
  if (policy.schema !== "simplicio.no-internal-json/v1" || policy.version !== 1 || !policy.scanner_version) throw new Error("unsupported policy schema/version");
  const seen = new Set();
  for (const item of policy.exceptions) {
    for (const key of ["path", "category", "owner", "external_dependency", "justification", "review_date", "removal_date"]) if (!item[key]) throw new Error(`exception missing ${key}`);
    if (seen.has(item.path) || item.path.startsWith("/") || item.path.split(/[\\/]/).includes("..") || /[*?\[\]]/.test(item.path)) throw new Error(`exception path is not exact: ${item.path}`);
    seen.add(item.path);
    if (!datePattern.test(item.review_date) || !datePattern.test(item.removal_date)) throw new Error(`invalid exception dates: ${item.path}`);
    if (item.removal_date < today) throw new Error(`expired exception: ${item.path}`);
  }
  return policy;
}

function files(root) {
  const result = [];
  let entries;
  try {
    entries = fs.readdirSync(root, { withFileTypes: true }).sort((left, right) => compareText(left.name, right.name));
  } catch {
    return result;
  }
  for (const entry of entries) {
    const full = path.join(root, entry.name);
    if (entry.isDirectory()) {
      if (!ignored.has(entry.name) && !entry.name.startsWith(".")) result.push(...files(full));
    } else if (entry.isFile()) {
      result.push(full);
    } else if (entry.isSymbolicLink()) {
      try {
        if (!fs.statSync(full).isDirectory()) result.push(full);
      } catch {
        result.push(full);
      }
    }
  }
  return result;
}

function scan(root, policy) {
  const exceptions = new Map(policy.exceptions.map((item) => [item.path, item.category]));
  const findings = [];
  for (const full of files(root)) {
    const relative = path.relative(root, full).split(path.sep).join("/");
    const category = exceptions.get(relative) ?? "unclassified";
    const extension = path.extname(full).toLowerCase();
    if (jsonExtensions.has(extension)) findings.push([relative, 1, "artifact-extension", extension.slice(1), category]);
    let data;
    try {
      data = fs.readFileSync(full);
    } catch {
      continue;
    }
    if (data.length > 4 * 1024 * 1024 || data.includes(0)) continue;
    let text;
    try {
      text = new TextDecoder("utf-8", { fatal: true }).decode(data);
    } catch {
      continue;
    }
    const trimmed = text.trim();
    if (!jsonExtensions.has(extension) && trimmed.startsWith("{") && trimmed.endsWith("}")) findings.push([relative, 1, "renamed-json-artifact", "object-document", category]);
    text.split(/\r\n|[\n\v\f\r\x1c-\x1e\x85\u2028\u2029]/).forEach((line, index) => patterns.forEach(([kind, needle]) => {
      if (line.includes(needle)) findings.push([relative, index + 1, kind, needle, category]);
    }));
  }
  const unique = [...new Set(findings.map((item) => JSON.stringify(item)))].map((item) => JSON.parse(item));
  return unique.sort((left, right) => compareFinding(left, right));
}

function compareFinding(left, right) {
  const pathOrder = compareText(left[0], right[0]);
  if (pathOrder) return pathOrder;
  if (left[1] !== right[1]) return left[1] - right[1];
  for (const index of [2, 3, 4]) {
    const order = compareText(left[index], right[index]);
    if (order) return order;
  }
  return 0;
}

function render(findings, policy, mode) {
  const unclassified = findings.filter((item) => item[4] === "unclassified").length;
  const status = mode === "strict" && unclassified ? "FAIL" : (unclassified ? "UNVERIFIED" : "PASS");
  const lines = ["# No-internal-JSON policy scan", "", `- status: \`${status}\``, `- mode: \`${mode}\``, `- scanner_version: \`${policy.scanner_version}\``, `- findings: \`${findings.length}\``, `- unclassified: \`${unclassified}\``, "", "## Findings", "", "| Path | Line | Kind | Classification | Evidence |", "| --- | ---: | --- | --- | --- |"];
  for (const [file, line, kind, evidence, category] of findings) lines.push(`| \`${file}\` | ${line} | \`${kind}\` | \`${category}\` | \`${evidence}\` |`);
  const markdown = `${lines.join("\n")}\n`;
  const payload = `mode=${mode};status=${status};policy_version=${policy.version};scanner_version=${policy.scanner_version};findings=${findings.length};unclassified=${unclassified}`;
  const fields = ["0", "genesis", "policy-scan", payload, `policy-scanner:${policy.scanner_version}`, ""];
  const encoded = fields.map((field) => { const bytes = Buffer.from(field); const length = Buffer.alloc(8); length.writeBigUInt64LE(BigInt(Array.from(field).length)); return Buffer.concat([length, bytes]); });
  const hash = crypto.createHash("sha256").update(Buffer.concat(encoded)).digest("hex");
  const hbp = `schema=simplicio.hbp/v1\nversion=1.0.0\nseq=0\nprev_hash=genesis\ntopic=policy-scan\npayload=${payload}\nprovenance=${fields[4]}\nhash=${hash}\n`;
  return [markdown, hbp, status === "FAIL" ? 1 : 0];
}

const args = process.argv.slice(2);
const value = (flag, fallback) => { const index = args.indexOf(flag); return index < 0 ? fallback : args[index + 1]; };
const root = path.resolve(value("--repo", "."));
const policyPath = path.resolve(value("--policy", path.join(root, "policy/no-internal-json.toml")));
const mode = value("--mode", "baseline");
if (!["baseline", "strict"].includes(mode)) throw new Error("--mode must be baseline or strict");
const today = process.env.SIMPLICIO_POLICY_SCAN_DATE ?? new Date().toISOString().slice(0, 10);
const policy = policyFromToml(fs.readFileSync(policyPath, "utf8"), today);
const [markdown, hbp, code] = render(scan(root, policy), policy, mode);
const markdownPath = value("--markdown", null);
if (markdownPath) fs.writeFileSync(markdownPath, markdown); else process.stdout.write(markdown);
const hbpPath = value("--hbp", null);
if (hbpPath) fs.writeFileSync(hbpPath, hbp);
process.exitCode = code;
