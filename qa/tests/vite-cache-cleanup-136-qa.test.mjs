// Independent #136 / PRD §7 NFR QA. Execute the submitted teardown callbacks,
// with deterministic filesystem faults; no app code or native Windows lock is mocked as tested.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import vm from "node:vm";
import ts from "typescript";

const suites = ["csp-43-qa.test.mjs", "row-ids-q7-85-qa.test.mjs"];

function teardown(name) {
  const text = readFileSync(new URL(name, import.meta.url), "utf8");
  const ast = ts.createSourceFile(name, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
  assert.equal(ast.parseDiagnostics.length, 0);
  const hooks = ast.statements.filter((node) => ts.isExpressionStatement(node)
    && ts.isCallExpression(node.expression) && node.expression.expression.getText(ast) === "after");
  assert.equal(hooks.length, 1, `${name}: expected exactly one teardown hook`);
  return hooks[0].expression.arguments[0].getText(ast);
}

function context(overrides = {}) {
  return {
    scratch: "/unused/scratch", cache: "/unused/cache",
    server: { close: async () => {} }, dom: { window: { close() {} } },
    window: {}, document: {}, IS_REACT_ACT_ENVIRONMENT: true,
    rm: async () => {}, ...overrides,
  };
}

// A fresh process lets fs.rmdir be faulted before Node lazily captures it for recursive rm.
// The real fs.promises.rm, submitted hook and retry options run unchanged.
const filesystemCase = String.raw`
import assert from "node:assert/strict";
import fs from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import vm from "node:vm";
const [hook, code, mode] = process.argv.slice(1);
const dir = fs.mkdtempSync(join(tmpdir(), "queue-136-QA café 日本語 & "));
const cache = join(dir, "vite", "deps_temp_136");
fs.mkdirSync(cache, { recursive: true });
fs.writeFileSync(join(cache, "日本語 é [4K] & chunk.js"), "optimizer output");
const original = fs.rmdir;
let failures = 0;
fs.rmdir = function(path, ...args) {
  if (String(path) === dir && (mode === "persistent" || failures < 4)) {
    failures++;
    const callback = args.at(-1);
    return queueMicrotask(() => callback(Object.assign(new Error("injected " + code), { code })));
  }
  return original.call(this, path, ...args);
};
try {
  const { rm } = await import("node:fs/promises");
  const cleanup = vm.runInNewContext("(" + hook + ")", {
    scratch: dir, cache: dir, rm,
    server: { close: async () => {} }, dom: { window: { close() {} } },
  });
  if (mode === "persistent" || mode === "without-retries") {
    await assert.rejects(cleanup(), { code });
    assert.ok(fs.existsSync(dir), "failed cleanup must remain observable");
    if (mode === "persistent") assert.equal(failures, 11, "initial try plus ten retries");
  } else {
    await cleanup();
    assert.equal(failures, 4, "fault injector must actually exercise the retry path");
    assert.equal(fs.existsSync(dir), false, "all nested optimizer output must be removed");
    await cleanup(); // force:true tolerates an already-removed cache.
  }
} finally {
  fs.rmdir = original;
  fs.rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
}
`;

function runFilesystem(hook, code, mode) {
  const run = spawnSync(process.execPath, ["--input-type=module", "--eval", filesystemCase, hook, code, mode], {
    encoding: "utf8", timeout: 12000,
  });
  assert.equal(run.error, undefined, run.error?.message);
  assert.equal(run.status, 0, `${run.stdout}\n${run.stderr}`);
}

for (const name of suites) {
  const hook = teardown(name);

  test(`#136 QA: ${name} awaits server close and recursive removal with bounded retries`, async () => {
    let releaseClose, releaseRemoval, removed = false, settled = false;
    const closing = new Promise((resolve) => { releaseClose = resolve; });
    const removing = new Promise((resolve) => { releaseRemoval = resolve; });
    const cleanup = vm.runInNewContext(`(${hook})`, context({
      server: { close: () => closing },
      rm: async (path, options) => {
        removed = true;
        assert.equal(path, name.startsWith("csp") ? "/unused/scratch" : "/unused/cache");
        assert.equal(options.recursive, true);
        assert.equal(options.force, true);
        assert.ok(Number.isInteger(options.maxRetries) && options.maxRetries > 0);
        assert.ok(Number.isInteger(options.retryDelay) && options.retryDelay > 0);
        assert.ok(options.retryDelay * options.maxRetries * (options.maxRetries + 1) / 2 < 30000,
          "retry budget must fit the repository's per-test timeout");
        await removing;
      },
    }));
    const pending = cleanup().then(() => { settled = true; });
    assert.equal(removed, false, "do not delete a running optimizer's cache");
    releaseClose();
    await new Promise(setImmediate); // Drain the vm realm's promise continuations too.
    assert.equal(removed, true);
    assert.equal(settled, false, "the after hook must await deletion");
    releaseRemoval();
    await pending;
  });

  test(`#136 QA: ${name} tolerates setup failing before cache/server creation`, async () => {
    await vm.runInNewContext(`(${hook})`, context({
      scratch: undefined, cache: undefined, server: undefined,
      rm() { assert.fail("no cache was created"); },
    }))();
  });

  for (const code of ["ENOTEMPTY", "EBUSY", "EPERM"]) {
    test(`#136 QA: ${name} recovers from transient ${code} on Unicode cache paths`, () => {
      runFilesystem(hook, code, "transient");
    });
  }

  test(`#136 QA: ${name} reports a persistent lock after exhausting its retry budget`, () => {
    runFilesystem(hook, "EBUSY", "persistent");
  });

  test(`#136 QA: ${name} negative control fails on ENOTEMPTY without retries`, () => {
    const withoutRetries = hook.replace(/, maxRetries: \d+, retryDelay: \d+/, "");
    assert.notEqual(withoutRetries, hook, "negative control must remove retry options");
    runFilesystem(withoutRetries, "ENOTEMPTY", "without-retries");
  });

  for (const option of ["maxRetries", "retryDelay"]) {
    test(`#136 QA: the submitted guard catches ${name} losing ${option}`, (t) => {
      const dir = mkdtempSync(join(tmpdir(), "queue-136-guard-"));
      t.after(() => rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 }));
      for (const file of [...suites, "vite-cache-cleanup-136.test.mjs"]) {
        copyFileSync(new URL(file, import.meta.url), join(dir, file));
      }
      // Mutate only a disposable copy; the actual submitted files remain untouched.
      const file = join(dir, name);
      const source = readFileSync(file, "utf8");
      const mutated = source.replace(new RegExp(`, ${option}: \\d+`), "");
      assert.notEqual(mutated, source);
      // Dynamic import keeps the fixture writer distinct from the teardown under test.
      return import("node:fs/promises").then(async ({ writeFile }) => {
        await writeFile(file, mutated);
        const env = { ...process.env };
        delete env.NODE_TEST_CONTEXT;
        const run = spawnSync(process.execPath, ["--test", "--test-reporter=tap", join(dir, "vite-cache-cleanup-136.test.mjs")], {
          encoding: "utf8", timeout: 5000, env,
        });
        assert.equal(run.error, undefined, run.error?.message);
        assert.equal(run.status, 1, `${run.stdout}\n${run.stderr}`);
        assert.ok(run.stdout.includes(`${name}:`), run.stdout);
        assert.ok(run.stdout.includes(`has no ${option}`), run.stdout);
      });
    });
  }
}
