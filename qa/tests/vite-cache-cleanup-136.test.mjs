// #136: a test that gives Vite a temp cacheDir must remove it with rm's retries. On Windows,
// Vite's optimizer can still hold files in deps_temp_* after server.close(), and a plain
// recursive rm then fails the run with ENOTEMPTY even though every test passed.
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { test } from "node:test";

const dir = new URL("./", import.meta.url);
const files = readdirSync(dir).filter((name) => name.endsWith(".mjs"));
const usesTempCache = (src) => /\bcacheDir\s*:/.test(src) && /\bmkdtemp\b/.test(src);

test("#136: every QA test with a temp Vite cache removes it with retries", () => {
  const checked = [];
  for (const name of files) {
    const src = readFileSync(new URL(name, dir), "utf8");
    if (!usesTempCache(src)) continue;
    checked.push(name);
    const removals = src.match(/\brm\([^)]*recursive: true[^)]*\)/g) ?? [];
    assert.ok(removals.length > 0, `${name}: no recursive rm of its temp folder`);
    for (const call of removals) {
      assert.match(call, /maxRetries: [1-9]\d*/, `${name}: ${call} has no maxRetries`);
      assert.match(call, /retryDelay: [1-9]\d*/, `${name}: ${call} has no retryDelay`);
    }
  }
  // Both known files must be found, so a broken detector can't pass by checking nothing.
  assert.ok(checked.includes("csp-43-qa.test.mjs"), checked.join(", "));
  assert.ok(checked.includes("row-ids-q7-85-qa.test.mjs"), checked.join(", "));
});
