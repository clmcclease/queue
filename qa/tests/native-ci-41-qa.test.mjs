// #41 / PRD §7: installed sidecars must have an opt-in loopback test invocation in CI.
// This checks the missing execution path, rather than treating `#[ignore]` as coverage.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

test("#41 CI provides an opt-in invocation of the real-sidecar loopback download tests", () => {
  const workflow = readFileSync(new URL("../../.github/workflows/build.yml", import.meta.url), "utf8");
  const steps = workflow.replaceAll("\r\n", "\n").split(/^      - /m);
  const loopback = steps.filter(step =>
    /cargo test\b/.test(step) && /--test[ =]+download_real_d2\b/.test(step) && /--ignored\b/.test(step));
  assert.ok(loopback.length > 0,
    "cargo test --locked skips the ignored download_real_d2 tests; #41 needs a guarded real-sidecar invocation");
  for (const step of loopback) {
    assert.match(step, /QUEUE_QA_REAL_SIDECARS/, "the tests need the per-OS installed binaries directory");
    assert.doesNotMatch(step, /QUEUE_QA_NETWORK:\s*["']?1/, "CI loopback tests must not opt into external YouTube downloads");
  }
});
