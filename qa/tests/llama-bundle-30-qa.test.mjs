// Independent QA for N6 / #30(a): release pins and the resource bundle contract.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const root = new URL("../../", import.meta.url);
const read = (path) => readFileSync(new URL(path, root), "utf8");
const manifest = JSON.parse(read("scripts/sidecars.json")).llama;
const targets = ["aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"];

test("#30(a): all four llama archives are pinned to official release assets and sha256", () => {
  assert.match(manifest.version, /^b\d+$/);
  assert.equal(manifest.url, "https://github.com/ggml-org/llama.cpp/releases/download/{version}/{asset}");
  assert.deepEqual(Object.keys(manifest.assets).sort(), [...targets].sort());
  for (const [target, pin] of Object.entries(manifest.assets)) {
    assert.match(pin.sha256, /^[a-f0-9]{64}$/, target);
    assert.ok(pin.asset.startsWith(`llama-${manifest.version}-bin-`), target);
    assert.match(pin.asset, /\.(tar\.gz|zip)$/);
    assert.ok(!pin.asset.includes("/"));
    assert.ok(!pin.prefix.startsWith("/") && !pin.prefix.includes(".."));
    const names = Object.keys(pin.files);
    assert.ok(names.includes(target.includes("windows") ? "llama-server.exe" : "llama-server"), target);
    assert.ok(names.some((name) => /(?:lib)?llama(?:\.so|\.\d+\.dylib|\.dll)/.test(name)), `${target}: inference library`);
    assert.ok(names.some((name) => /ggml-cpu/.test(name)), `${target}: CPU fallback`);
    for (const [name, entry] of Object.entries(pin.files)) {
      // All runtime files must remain siblings; archive paths must never escape staging.
      assert.ok(name && !/[\\/:]/.test(name) && name !== "." && name !== "..", name);
      assert.ok(entry && !/[\\/:]/.test(entry) && entry !== "." && entry !== "..", entry);
    }
    // llama.cpp's MIT license on every target: from the archive, or (Windows) from the repo.
    const repoFiles = pin.repoFiles ?? {};
    assert.ok(names.includes("LICENSE") || "LICENSE" in repoFiles, `${target}: llama.cpp's LICENSE is bundled`);
    for (const path of Object.values(repoFiles)) {
      const text = readFileSync(new URL(`../../${path}`, import.meta.url), "utf8");
      assert.match(text, /^MIT License/);
      assert.match(text, /Copyright \(c\) 2023-2026 The ggml authors/);
      assert.match(text, /Permission is hereby granted, free of charge/);
    }
    if (target.includes("windows")) {
      assert.ok(names.includes("libomp.dll"));
      assert.ok(names.includes("LICENSE-LLVM-OpenMP"));
      assert.match(pin.asset, /win-cpu-x64\.zip$/);
    }
    if (target === "aarch64-apple-darwin") assert.ok(names.some((name) => name.includes("ggml-metal")));
  }
});

test("#30(a): Tauri keeps the runtime and dynamic libraries together as resources", () => {
  const config = JSON.parse(read("src-tauri/tauri.conf.json"));
  assert.equal(config.bundle.resources["binaries/llama/"], "llama/");
  assert.ok(!config.bundle.externalBin.some((name) => name.includes("llama")), "an externalBin would separate/rename the server");
  for (const library of ["libssl3", "libgomp1", "libstdc++6"]) {
    assert.ok(config.bundle.linux.deb.depends.includes(library), library);
  }
});
