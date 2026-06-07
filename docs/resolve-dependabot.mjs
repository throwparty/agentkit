#!/usr/bin/env node
import { readFileSync, writeFileSync } from "fs";
import { execSync } from "child_process";
import { resolve, dirname } from "path";
import { fileURLToPath } from "url";

const HERE = dirname(fileURLToPath(import.meta.url));
const YARN = ".yarn/releases/yarn-4.15.0.cjs";

// ── parse arguments ─────────────────────────────────────────────
const input = process.argv[2];
if (!input) {
  console.log("Usage: resolve-dependabot.mjs <alert-number-or-url>\n");
  console.log("Requires: node, gh CLI authenticated");
  process.exit(1);
}

const num = input.match(/^(\d+)$/)?.[1] ?? input.match(/\/(\d+)$/)?.[1];
if (!num) {
  console.log("Could not parse alert number from:", input);
  process.exit(1);
}

// ── helpers ─────────────────────────────────────────────────────
function $(cmd) {
  return execSync(cmd, { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] }).trim();
}

function cmp(a, b) {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let i = 0; i < 3; i++) {
    const va = pa[i] || 0;
    const vb = pb[i] || 0;
    if (va !== vb) return va - vb;
  }
  return 0;
}

function parseVulnRange(r) {
  const c = {};
  for (const p of r.split(",").map(s => s.trim())) {
    const m = p.match(/^([<>=]+)\s*(.+)$/);
    if (!m) continue;
    c[m[1]] = m[2];
  }
  return c;
}

function isVuln(ver, c) {
  if (c[">="] && cmp(ver, c[">="]) < 0) return false;
  if (c[">"]  && cmp(ver, c[">"])  <= 0) return false;
  if (c["<="] && cmp(ver, c["<="]) > 0) return false;
  if (c["<"]  && cmp(ver, c["<"])  >= 0) return false;
  return true;
}

// ── fetch alert ─────────────────────────────────────────────────
console.log("[1/4] Fetching Dependabot alert #" + num + " …");
const raw = $(`gh api repos/${$("gh repo view --json nameWithOwner --jq .nameWithOwner")}/dependabot/alerts/${num}`);
const alert = JSON.parse(raw);
const pkg = alert.dependency.package;

if (pkg.ecosystem !== "npm") {
  console.log("Unsupported ecosystem: " + pkg.ecosystem);
  process.exit(1);
}

const pkgName = pkg.name;
const vulnRange = alert.security_vulnerability.vulnerable_version_range;
const firstPatched = alert.security_vulnerability.first_patched_version.identifier;

console.log("  Package:       " + pkgName);
console.log("  Vulnerable:    " + vulnRange);
console.log("  First patched: " + firstPatched);

// ── scan lockfile ───────────────────────────────────────────────
const yarnlock = readFileSync(resolve(HERE, "yarn.lock"), "utf8");
const pj = JSON.parse(readFileSync(resolve(HERE, "package.json"), "utf8"));

console.log("[2/4] Scanning yarn.lock for vulnerable resolutions …");

const constraints = parseVulnRange(vulnRange);
const esc = pkgName.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const re = new RegExp('"' + esc + '@npm:([^"]+)":\\n  version: ([^\\n]+)', "g");
const toAdd = [];

let m;
while ((m = re.exec(yarnlock)) !== null) {
  const rawRanges = m[1];
  const resolvedVer = m[2].trim();
  if (!isVuln(resolvedVer, constraints)) continue;

  for (const r of rawRanges.split(",").map(s => s.trim())) {
    const rangeKey = pkgName + "@" + r;
    const rangeVal = "^" + firstPatched;
    toAdd.push([rangeKey, rangeVal]);
    console.log("  " + rangeKey + " → " + rangeVal + "  (was " + resolvedVer + ")");
  }
}

if (toAdd.length === 0) {
  console.log("  No vulnerable resolutions found.");
  process.exit(0);
}

if (!pj.resolutions) pj.resolutions = {};
for (const [k, v] of toAdd) pj.resolutions[k] = v;
writeFileSync("package.json", JSON.stringify(pj, null, 2) + "\n");
console.log("\n  → " + toAdd.length + " resolution(s) written to package.json");

// ── apply ───────────────────────────────────────────────────────
console.log("\n[3/4] Applying resolutions with yarn install …");
execSync("node " + YARN + " install", { stdio: "inherit", cwd: HERE });

// ── verify ──────────────────────────────────────────────────────
console.log("\n[4/4] Verifying lockfile …");
const updated = readFileSync(resolve(HERE, "yarn.lock"), "utf8");
for (const line of updated.split("\n")) {
  if (line.includes(pkgName + "@npm:")) console.log(line);
}
console.log("\nDone.");
