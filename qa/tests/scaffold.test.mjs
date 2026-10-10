// Issue #1 (PRD §8; no Q/D/N ID): run after npm ci and npm run build.
// No additional dependencies or production-file changes are needed.
// Run: node --test qa/tests/scaffold.test.mjs
import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import test from "node:test";
import { fileURLToPath } from "node:url";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";
import { resolveConfig } from "vite";

const root = new URL("../../", import.meta.url);
const read = (path) => readFile(new URL(path, root), "utf8");
const json = async (path) => JSON.parse(await read(path));
const exactVersion = /^\d+\.\d+\.\d+(?:-[\w.-]+)?$/;

async function renderApp() {
  const { outputText } = ts.transpileModule(await read("src/App.tsx"), {
    compilerOptions: {
      jsx: ts.JsxEmit.ReactJSX,
      module: ts.ModuleKind.ESNext,
      target: ts.ScriptTarget.ES2020,
    },
  });
  // Resolve the JSX runtime from the project, since data URLs lack a base path.
  const code = outputText.replaceAll(
    '"react/jsx-runtime"',
    JSON.stringify(import.meta.resolve("react/jsx-runtime")),
  );
  const { default: App } = await import(
    `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
  );
  return renderToStaticMarkup(
    React.createElement(React.StrictMode, null, React.createElement(App)),
  );
}

test("the initial React screen renders a readable empty state", async () => {
  const html = await renderApp();
  assert.match(html, /^<main\b/);
  assert.match(html, /<p\b[^>]*>Queue is empty\.<\/p>/);
  assert.doesNotMatch(html, /<(?:input|button|a|form)\b/);
});

test("production Tailwind output contains the empty screen and system dark theme styles", async () => {
  const files = await readdir(new URL("dist/assets/", root));
  const cssFiles = files.filter((file) => file.endsWith(".css"));
  assert.ok(cssFiles.length > 0, "run npm run build before this suite");
  const css = (await Promise.all(cssFiles.map((file) => read(`dist/assets/${file}`)))).join("\n");
  for (const selector of [".h-screen", ".items-center", ".justify-center", ".bg-white", ".dark\\:bg-neutral-900"]) {
    assert.ok(css.includes(selector), `missing compiled style ${selector}`);
  }
  assert.match(css, /prefers-color-scheme:\s*dark/);
});

test("Node and npm dependency pins agree with mise and the lockfile", async () => {
  const pkg = await json("package.json");
  const lock = await json("package-lock.json");
  assert.match(pkg.engines.node, exactVersion);
  assert.equal((await read("mise.toml")).match(/^node\s*=\s*"([^"]+)"/m)?.[1], pkg.engines.node);
  const npmrc = await read(".npmrc");
  assert.match(npmrc, /^engine-strict=true$/m);
  assert.match(npmrc, /^save-exact=true$/m);
  for (const group of ["dependencies", "devDependencies"]) {
    assert.deepEqual(lock.packages[""][group], pkg[group]);
    for (const [name, version] of Object.entries(pkg[group])) {
      assert.match(version, exactVersion, `${name} must have an exact version`);
      assert.equal(lock.packages[`node_modules/${name}`]?.version, version, `${name} lock mismatch`);
    }
  }
});

test("Rust toolchain and direct Tauri crate versions agree with the manifest and lockfile", async () => {
  const toolchain = await read("rust-toolchain.toml");
  const manifest = await read("src-tauri/Cargo.toml");
  const lock = await read("src-tauri/Cargo.lock");
  const channel = toolchain.match(/^channel\s*=\s*"([^"]+)"/m)?.[1];
  assert.match(channel ?? "", exactVersion);
  assert.equal(manifest.match(/^rust-version\s*=\s*"([^"]+)"/m)?.[1], channel);
  assert.match(toolchain, /components\s*=\s*\[[^\]]*"clippy"/);
  for (const name of ["tauri", "tauri-build"]) {
    const version = manifest.match(new RegExp(`^${name}\\s*=\\s*\\{\\s*version\\s*=\\s*"=([^"\\s]+)"`, "m"))?.[1];
    assert.match(version ?? "", exactVersion, `${name} must have an exact crate pin`);
    const entry = lock.split("[[package]]").find((block) => block.includes(`\nname = "${name}"\n`));
    assert.ok(entry, `${name} missing from Cargo.lock`);
    assert.equal(entry.match(/\nversion = "([^"]+)"/)?.[1], version);
  }
});

test("Vite and Tauri share a strict development port and enable React and Tailwind", async () => {
  const tauri = await json("src-tauri/tauri.conf.json");
  const pkg = await json("package.json");
  const vite = await resolveConfig({ root: fileURLToPath(root) }, "serve");
  assert.equal(tauri.build.beforeDevCommand, "npm run dev");
  assert.equal(pkg.scripts.dev, "vite");
  assert.equal(pkg.scripts.tauri, "tauri");
  const url = new URL(tauri.build.devUrl);
  assert.equal(Number(url.port), vite.server.port);
  assert.equal(vite.server.strictPort, true, "port conflicts must fail rather than silently change ports");
  assert.ok(vite.plugins.some((plugin) => plugin.name.includes("react")));
  assert.ok(vite.plugins.some((plugin) => plugin.name.includes("tailwindcss")));
});

test("the built frontend directory agrees with Tauri's bundle configuration", async () => {
  const tauri = await json("src-tauri/tauri.conf.json");
  const vite = await resolveConfig({ root: fileURLToPath(root) }, "build");
  const tauriDist = new URL(`${tauri.build.frontendDist}/`, new URL("src-tauri/", root));
  const viteDist = new URL(`${vite.build.outDir}/`, root);
  assert.equal(tauriDist.href, viteDist.href);
  const html = await readFile(new URL("index.html", tauriDist), "utf8");
  assert.match(html, /id="root"/);
  assert.match(html, /<script[^>]+type="module"[^>]+src="[^"]+\.js"/);
  assert.equal(tauri.build.beforeBuildCommand, "npm run build");
});
