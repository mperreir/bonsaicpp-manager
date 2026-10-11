// Checks that every locale in src/i18n/*.json has exactly the same keys
// as the reference (fr.json). Run: `npm run i18n:check`.
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const dir = join(dirname(fileURLToPath(import.meta.url)), "..", "src", "i18n");

function flat(obj, prefix = "") {
  const out = [];
  for (const [k, v] of Object.entries(obj)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === "object") out.push(...flat(v, key));
    else out.push(key);
  }
  return out.sort();
}

const files = readdirSync(dir).filter((f) => f.endsWith(".json")).sort();
if (!files.includes("fr.json")) {
  console.error("i18n:check: missing reference src/i18n/fr.json");
  process.exit(1);
}
const ref = new Set(flat(JSON.parse(readFileSync(join(dir, "fr.json"), "utf8"))));
let failed = false;
for (const f of files) {
  if (f === "fr.json") continue;
  const keys = new Set(flat(JSON.parse(readFileSync(join(dir, f), "utf8"))));
  for (const k of ref) {
    if (!keys.has(k)) {
      console.error(`i18n:check: ${f} misses key "${k}"`);
      failed = true;
    }
  }
  for (const k of keys) {
    if (!ref.has(k)) {
      console.error(`i18n:check: ${f} has extra key "${k}"`);
      failed = true;
    }
  }
}
if (failed) process.exit(1);
console.log(`i18n:check: OK (${files.join(", ")}, ${ref.size} keys)`);
