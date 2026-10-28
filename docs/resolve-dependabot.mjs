#!/usr/bin/env node
import { readFileSync, writeFileSync } from "fs";
import { execSync } from "child_process";
import { resolve, dirname } from "path";
import { fileURLToPath } from "url";

const HERE = dirname(fileURLToPath(import.meta.url));

function getYarnPath() {
  try {
    const yarnrc = readFileSync(resolve(HERE, ".yarnrc.yml"), "utf8");
    const m = yarnrc.match(/^\s*yarnPath:\s*["']?([^"'\n]+)["']?\s*$/m);
    if (m?.[1]) return m[1];
  } catch {
    // fall back to default below
  }
  return ".yarn/releases/yarn-4.15.0.cjs";
}

const YARN = getYarnPath();

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
  return execSync(cmd, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  }).trim();
}

function cmp(a, b) {
  const parseCore = (v) =>
    (v.split(/[+-]/, 1)[0].match(/^\d+(?:\.\d+){0,2}/)?.[0] ?? "")
      .split(".")
      .map((n) => Number(n) || 0);

  const pa = parseCore(a);
  const pb = parseCore(b);
  for (let i = 0; i < 3; i++) {
    const va = pa[i] || 0;
    const vb = pb[i] || 0;
    if (va !== vb) return va - vb;
  }
  return 0;
}

function parseVulnRange(r) {
  const c = {};
  for (const p of r.split(",").map((s) => s.trim())) {
    const m = p.match(/^([<>=]+)\s*(.+)$/);
    if (!m) continue;
    c[m[1]] = m[2];
  }
  return c;
}

function isVuln(ver, c) {
  if (c[">="] && cmp(ver, c[">="]) < 0) return false;
  if (c[">"] && cmp(ver, c[">"]) <= 0) return false;
  if (c["<="] && cmp(ver, c["<="]) > 0) return false;
  if (c["<"] && cmp(ver, c["<"]) >= 0) return false;
  return true;
}

// ── fetch alert ─────────────────────────────────────────────────
console.log("[1/4] Fetching Dependabot alert #" + num + " …");
const raw = $(
  `gh api repos/${$("gh repo view --json nameWithOwner --jq .nameWithOwner")}/dependabot/alerts/${num}`,
);
const alert = JSON.parse(raw);
const pkg = alert.dependency.package;

if (pkg.ecosystem !== "npm") {
  console.log("Unsupported ecosystem: " + pkg.ecosystem);
  process.exit(1);
}

const pkgName = pkg.name;
const vulnRange = alert.security_vulnerability.vulnerable_version_range;
const firstPatched =
  alert.security_vulnerability.first_patched_version?.identifier;
if (!firstPatched) {
  console.log(
    "No first patched version is available for alert #" +
      num +
      "; cannot safely auto-resolve.",
  );
  process.exit(1);
}

console.log("  Package:       " + pkgName);
console.log("  Vulnerable:    " + vulnRange);
console.log("  First patched: " + firstPatched);

// ── scan lockfile ───────────────────────────────────────────────
const yarnlock = readFileSync(resolve(HERE, "yarn.lock"), "utf8");
const pj = JSON.parse(readFileSync(resolve(HERE, "package.json"), "utf8"));

console.log("[2/4] Scanning yarn.lock for vulnerable resolutions …");

const constraints = parseVulnRange(vulnRange);
const esc = pkgName.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

// Lockfile keys hold the *resolved* descriptors, which may already have been
// rewritten by an existing `resolutions` entry. Map every resolved range to
// its version so declared ranges can be followed through the chain.
const resolvedVersions = new Map();
const entryRe = new RegExp(
  '^"' + esc + '@npm:([^"]+)":\\n  version: ([^\\n]+)',
  "gm",
);
const prefix = new RegExp("^" + esc + "@npm:");
let m;
while ((m = entryRe.exec(yarnlock)) !== null) {
  for (const r of m[1].split(",").map((s) => s.trim())) {
    resolvedVersions.set(r.replace(prefix, ""), m[2].trim());
  }
}

// Ranges as declared by dependents, before any resolution rewrote them.
// Yarn quotes keys only when they would be ambiguous, which includes every
// scoped name (`"@scope/pkg": "npm:1.0.0"`), so accept both forms.
const declaredRanges = new Set();
const declRe = new RegExp(
  '^\\s+(?:"' + esc + '"|' + esc + '): "npm:([^"]+)"$',
  "gm",
);
while ((m = declRe.exec(yarnlock)) !== null) {
  declaredRanges.add(m[1].trim());
}

// Lockfile keys carry ranges (`^5.0.9`), resolution values carry ranges too,
// so prefer the exact form and fall back to a bare version.
function versionFor(range) {
  return (
    resolvedVersions.get(range) ??
    resolvedVersions.get(range.replace(/^[\^~]/, ""))
  );
}

function resolveRange(range) {
  const seen = new Set();
  let current = range;
  while (!seen.has(current)) {
    seen.add(current);
    // A bare override (`"qs": "^6.16.0"`) rewrites every descriptor of the
    // package, so it applies when no descriptor-specific entry matches.
    const override =
      pj.resolutions?.[pkgName + "@" + current] ?? pj.resolutions?.[pkgName];
    if (!override) break;
    current = override;
  }
  return versionFor(current);
}

const toAdd = [];
for (const range of declaredRanges) {
  // A bare override already covers every descriptor, so update that instead
  // of growing the resolutions list with a redundant entry.
  if (pj.resolutions?.[pkgName]) continue;

  const resolvedVer = resolveRange(range);
  if (resolvedVer === undefined || !isVuln(resolvedVer, constraints)) continue;

  const rangeKey = pkgName + "@" + range;
  const rangeVal = "^" + firstPatched;
  toAdd.push([rangeKey, rangeVal]);
  console.log(
    "  " + rangeKey + " → " + rangeVal + "  (was " + resolvedVer + ")",
  );
}

// Intermediate entries in an existing resolution chain would otherwise keep
// pointing at the vulnerable version, including bare-name overrides.
for (const [key, value] of Object.entries(pj.resolutions ?? {})) {
  if (key !== pkgName && !key.startsWith(pkgName + "@")) continue;
  const resolvedVer = versionFor(value);
  if (resolvedVer === undefined || !isVuln(resolvedVer, constraints)) continue;
  if (toAdd.some(([k]) => k === key)) continue;
  toAdd.push([key, "^" + firstPatched]);
  console.log("  " + key + " → ^" + firstPatched + "  (was " + value + ")");
}

if (toAdd.length === 0) {
  console.log("  No vulnerable resolutions found.");
  process.exit(0);
}

if (!pj.resolutions) pj.resolutions = {};
for (const [k, v] of toAdd) pj.resolutions[k] = v;
writeFileSync(
  resolve(HERE, "package.json"),
  JSON.stringify(pj, null, 2) + "\n",
);
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
