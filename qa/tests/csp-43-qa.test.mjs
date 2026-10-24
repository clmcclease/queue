// Independent #43 QA, PRD §7 (NFR), Q3/D4 and D-035.
// Builds the real frontend into a temporary directory. jsdom checks DOM write paths;
// it does NOT enforce CSP or replace the owner's native-webview checks.
import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";
import React, { act } from "react";
import ts from "typescript";
import { build, createServer } from "vite";

const project = fileURLToPath(new URL("../../", import.meta.url));
const config = JSON.parse(await readFile(join(project, "src-tauri/tauri.conf.json"), "utf8"));
function directives(policy) {
  const entries = policy.split(";").map((d) => d.trim().split(/\s+/)).filter(([d]) => d);
  assert.equal(new Set(entries.map(([d]) => d)).size, entries.length, "no duplicate directives");
  return Object.fromEntries(entries.map(([d, ...sources]) => [d, sources]));
}
const release = directives(config.app.security.csp);
const dev = directives(config.app.security.devCsp);
let scratch, html, assets, server, QueueList;
const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://localhost:1420" });
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createRoot } = await import("react-dom/client");

before(async () => {
  scratch = await mkdtemp(join(tmpdir(), "queue-csp-43-qa-"));
  const outDir = join(scratch, "dist");
  // Vite's build API sets NODE_ENV. Don't let that disable Fast Refresh in the dev check.
  const nodeEnv = process.env.NODE_ENV;
  try {
    await build({ root: project, configLoader: "runner", logLevel: "silent", build: { outDir, emptyOutDir: true } });
  } finally {
    if (nodeEnv === undefined) delete process.env.NODE_ENV;
    else process.env.NODE_ENV = nodeEnv;
  }
  html = await readFile(join(outDir, "index.html"), "utf8");
  assets = await Promise.all((await readdir(join(outDir, "assets"))).map(async (name) => ({
    name, text: await readFile(join(outDir, "assets", name), "utf8"),
  })));
  server = await createServer({
    root: project, configFile: false, cacheDir: join(scratch, "vite"),
    logLevel: "silent", appType: "custom",
    server: { middlewareMode: true, hmr: false, watch: null },
  });
  ({ default: QueueList } = await server.ssrLoadModule("/src/components/QueueList.tsx"));
});

after(async () => {
  await server?.close();
  dom.window.close();
  delete globalThis.window;
  delete globalThis.document;
  delete globalThis.IS_REACT_ACT_ENVIRONMENT;
  if (scratch) await rm(scratch, { recursive: true, force: true });
});

test("#43 QA: release HTML loads existing same-origin scripts and styles without inline content", () => {
  const page = new JSDOM(html, { url: "http://tauri.localhost/" });
  try {
    const document = page.window.document;
    assert.equal(document.querySelectorAll("style, [style], base, iframe, object, embed").length, 0);
    assert.ok(document.querySelectorAll("script").length > 0);
    assert.ok(document.querySelectorAll('link[rel="stylesheet"]').length > 0);
    for (const script of document.querySelectorAll("script")) {
      assert.equal(script.textContent.trim(), "");
      assert.ok(script.hasAttribute("src"));
    }
    for (const element of document.querySelectorAll("script[src], link[href]")) {
      const target = new URL(element.getAttribute("src") ?? element.getAttribute("href"), page.window.location.href);
      assert.equal(target.origin, page.window.location.origin);
      assert.ok(assets.some(({ name }) => target.pathname === `/assets/${name}`), target.href);
    }
    for (const element of document.querySelectorAll("*")) {
      assert.ok([...element.attributes].every(({ name }) => !/^on/i.test(name)), element.outerHTML);
    }
  } finally { page.window.close(); }
});

test("#43 QA: compiled JavaScript has no eval, Function constructor or string timer calls", () => {
  const scripts = assets.filter(({ name }) => /\.m?js$/.test(name));
  assert.ok(scripts.length > 0);
  for (const { name, text } of scripts) {
    const ast = ts.createSourceFile(name, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
    assert.equal(ast.parseDiagnostics.length, 0);
    function inspect(node) {
      if (ts.isCallExpression(node) || ts.isNewExpression(node)) {
        const callee = node.expression.getText(ast);
        assert.ok(!/\b(?:eval|Function)\b/.test(callee), `${name}: ${callee}`);
        if (/\bset(?:Timeout|Interval)$/.test(callee)) {
          assert.ok(!node.arguments?.[0] || !ts.isStringLiteralLike(node.arguments[0]), `${name}: string timer`);
        }
      }
      ts.forEachChild(node, inspect);
    }
    inspect(ast);
  }
});

test("#43 QA: release CSS is extracted and needs no remote stylesheet, image or font", () => {
  const styles = assets.filter(({ name }) => /\.css$/.test(name));
  assert.ok(styles.length > 0);
  for (const { text } of styles) {
    assert.doesNotMatch(text, /@import\b/i);
    for (const [, url] of text.matchAll(/url\(\s*["']?([^\s"')]+)/g)) {
      assert.ok(!/^(?:https?:)?\/\//i.test(url), `unexpected external CSS dependency: ${url}`);
    }
  }
});

test("#43 QA: development keeps release restrictions except the explicit Vite allowances", () => {
  assert.deepEqual(Object.keys(dev).sort(), Object.keys(release).sort());
  for (const [name, sources] of Object.entries(release)) {
    const added = name === "script-src" || name === "style-src" ? ["'unsafe-inline'"]
      : name === "connect-src" ? ["http://localhost:1420", "ws://localhost:1420"] : [];
    assert.deepEqual(dev[name], [...sources, ...added], name);
  }
  assert.deepEqual(release["form-action"], ["'self'"]);
  assert.deepEqual(release["font-src"], ["'self'", "data:"]);
  assert.ok(!release["connect-src"].some((source) => /https:|localhost:1420/.test(source)));
});

test("#43 QA: actual Vite/React dev HTML uses only self modules and the allowed inline refresh preamble", async () => {
  const devServer = await createServer({
    root: project, configLoader: "runner", logLevel: "silent",
    server: { middlewareMode: true, watch: null },
  });
  try {
    assert.equal(devServer.config.server.port, 1420);
    const page = new JSDOM(await devServer.transformIndexHtml("/", await readFile(join(project, "index.html"), "utf8")), {
      url: config.build.devUrl,
    });
    try {
      const scripts = [...page.window.document.querySelectorAll("script")];
      assert.ok(scripts.some((script) => /@vite\/client/.test(script.src)));
      assert.ok(scripts.some((script) => /@react-refresh/.test(script.textContent)));
      for (const script of scripts) {
        if (script.src) assert.equal(new URL(script.src).origin, new URL(config.build.devUrl).origin);
        else assert.ok(dev["script-src"].includes("'unsafe-inline'"));
      }
    } finally { page.window.close(); }
  } finally { await devServer.close(); }
});

const item = (overrides = {}) => ({
  id: 1, rev: 1, url: "https://example.test/watch?v=1", status: { state: "downloading" },
  name: "日本語 – café", attempt: 0,
  meta: { title: "  日本語 🎵 <script>bad()</script> CON & café  ", channel: "<b>Channel</b>", duration: 60,
    thumbnail: "https://cdn.example.test/日本語.jpg?sig=a%2Bb&size=100" },
  progress: { percent: 1, speed: 2048, eta: 10 }, ...overrides,
});
async function withRows(run) {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const render = async (items) => act(async () => root.render(React.createElement(QueueList, {
    items, connected: true, busy: true, cancelling: new Set(),
    onDownload() {}, onRetry() {}, onRename() {}, onRemove() {}, onCancel() {}, onReveal() {},
  })));
  try { await run(container, render); }
  finally { await act(async () => root.unmount()); container.remove(); }
}

test("#43 QA: progress style props use CSSOM width, never a style attribute setter or cssText", async () => {
  const element = dom.window.Element.prototype;
  const css = Object.getPrototypeOf(document.createElement("div").style);
  const cssTextOwner = Object.getPrototypeOf(css);
  const setAttribute = element.setAttribute;
  const cssText = Object.getOwnPropertyDescriptor(cssTextOwner, "cssText");
  const width = Object.getOwnPropertyDescriptor(css, "width");
  const widths = [];
  element.setAttribute = function (name, value) {
    assert.notEqual(name.toLowerCase(), "style", "CSP would reject this attribute write");
    return setAttribute.call(this, name, value);
  };
  Object.defineProperty(cssTextOwner, "cssText", { ...cssText, set() { assert.fail("cssText write"); } });
  Object.defineProperty(css, "width", { ...width, set(value) { widths.push(value); width.set.call(this, value); } });
  try {
    await withRows(async (container, render) => {
      for (const percent of [0, 1.5, 50, 99.9, 100]) {
        await render([item({ progress: { percent, speed: 1024, eta: 1 } })]);
        const bar = container.querySelector('[role="progressbar"]');
        assert.equal(bar.firstElementChild.style.width, `${percent}%`);
        assert.equal(bar.getAttribute("aria-valuenow"), String(Math.floor(percent)));
      }
    });
    assert.ok(widths.includes("1.5%") && widths.includes("100%"));
  } finally {
    element.setAttribute = setAttribute;
    Object.defineProperty(cssTextOwner, "cssText", cssText);
    Object.defineProperty(css, "width", width);
  }
});

test("#43 QA: messy Unicode metadata and failed-row messages remain text, with HTTPS thumbnails intact", async () => {
  await withRows(async (container, render) => {
    const downloading = item();
    const reason = '<img src=x onerror="bad()"> CON / 日本語';
    await render([downloading, item({ id: 2, status: { state: "failed", reason }, progress: null })]);
    assert.ok(container.textContent.includes(downloading.meta.title));
    assert.ok(container.textContent.includes(reason));
    assert.equal(container.querySelectorAll("script, style, iframe, [onerror], [onclick]").length, 0);
    const image = container.querySelector("img");
    assert.equal(image.getAttribute("src"), downloading.meta.thumbnail);
    assert.equal(image.getAttribute("referrerpolicy"), "no-referrer");
    assert.ok(release["img-src"].includes(new URL(image.src).protocol));
  });
});
