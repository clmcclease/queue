// #43 CSP probe. Build with `npm run tauri build -- --debug --no-bundle` (the release CSP, with
// Web Inspector on), run src-tauri/target/debug/queue, right-click > Inspect Element > Console,
// paste this file and press Enter. Read-only: it only calls queue_list. Prints a table and a
// PASS/FAIL line. macOS passed 9/9 on 2026-10-27; Windows and Linux are #71.
(async () => {
  const violations = [];
  const onViolation = (e) => violations.push({ directive: e.effectiveDirective || e.violatedDirective, uri: e.blockedURI || "inline" });
  // A network check counts as blocked only with a matching CSP violation; any other failure
  // (offline, a 404, a timeout) is inconclusive, not a pass.
  const blockedBy = (directive, url) =>
    violations.some((v) => v.directive.startsWith(directive) && v.uri.startsWith(url));
  const verdict = (outcome, directive, url) =>
    outcome === "failed" ? (blockedBy(directive, url) ? "blocked" : "inconclusive (failed without a CSP violation)") : outcome;
  document.addEventListener("securitypolicyviolation", onViolation);
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  const results = [];
  const check = (name, expected, actual, detail = "") =>
    results.push({ check: name, expected, actual, ok: expected === actual ? "✔" : "✖", detail });

  // 1. Inline script must not run.
  window.__cspInline = false;
  const inline = document.createElement("script");
  inline.textContent = "window.__cspInline = true";
  document.head.appendChild(inline);
  await wait(100);
  check("inline <script>", "blocked", window.__cspInline ? "ran" : "blocked");

  // 2. eval and string timers must throw or not run.
  let evalRan = false;
  try { evalRan = (0, eval)("1 + 1") === 2; } catch { evalRan = false; }
  check("eval()", "blocked", evalRan ? "ran" : "blocked", "console code may be exempt in some engines");
  let fnRan = false;
  try { fnRan = new Function("return 1")() === 1; } catch { fnRan = false; }
  check("new Function()", "blocked", fnRan ? "ran" : "blocked");

  // 3. Remote script must not load.
  const remote = await new Promise((resolve) => {
    const s = document.createElement("script");
    s.src = "https://example.com/csp-probe.js";
    s.onload = () => resolve("loaded");
    s.onerror = () => resolve("failed");
    document.head.appendChild(s);
    setTimeout(() => resolve("failed"), 5000);
  });
  await wait(100);
  check("remote <script>", "blocked", verdict(remote, "script-src", "https://example.com/csp-probe.js"));

  // 4. fetch to another origin must fail (connect-src).
  let fetched;
  try { await fetch("https://example.com/", { mode: "no-cors" }); fetched = "allowed"; } catch { fetched = "failed"; }
  await wait(100);
  check("fetch https://example.com", "blocked", verdict(fetched, "connect-src", "https://example.com/"));

  // 5. Inline <style> must not apply.
  const marker = document.createElement("div");
  marker.id = "csp-probe-marker";
  document.body.appendChild(marker);
  const style = document.createElement("style");
  style.textContent = "#csp-probe-marker { width: 123px; }";
  document.head.appendChild(style);
  await wait(100);
  check("inline <style>", "blocked", getComputedStyle(marker).width === "123px" ? "applied" : "blocked");
  marker.remove(); style.remove();

  // 6. https thumbnails load; plain http and other schemes don't.
  const img = (src) => new Promise((resolve) => {
    const i = new Image();
    i.onload = () => resolve("loaded");
    i.onerror = () => resolve("failed");
    i.src = src;
    setTimeout(() => resolve("failed"), 8000);
  });
  check("https thumbnail (i.ytimg.com)", "loaded", await img("https://i.ytimg.com/vi/jNQXAC9IVRw/default.jpg"));
  const httpImg = await img("http://i.ytimg.com/vi/jNQXAC9IVRw/default.jpg");
  await wait(100);
  check("http thumbnail", "blocked", verdict(httpImg, "img-src", "http://i.ytimg.com/"));

  // 7. IPC to the backend still works (read-only command).
  let ipc;
  try { const rows = await window.__TAURI_INTERNALS__.invoke("queue_list"); ipc = Array.isArray(rows) ? "works" : "odd reply"; }
  catch (e) { ipc = `failed: ${e}`; }
  check("IPC invoke(queue_list)", "works", ipc);

  // 8. The policy the page actually got.
  await wait(200);
  document.removeEventListener("securitypolicyviolation", onViolation);
  console.table(results);
  console.log("Violations reported by the webview:\n" + (violations.map((v) => `${v.directive} ← ${v.uri}`).join("\n") || "(none)"));
  const failed = results.filter((r) => r.ok !== "✔");
  console.log(failed.length ? `CSP PROBE: FAIL (${failed.map((r) => r.check).join(", ")})` : "CSP PROBE: PASS");
  return failed.length ? "FAIL" : "PASS";
})();
