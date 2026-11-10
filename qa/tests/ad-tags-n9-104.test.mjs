// #104 / N9: the advertising-tag list in Settings (add, edit, remove, reset). Real React DOM, mocked
// Tauri IPC. Then npm run build && npm test (or node --test qa/tests/ad-tags-n9-104.test.mjs).
import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";
import React, { act } from "react";
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks";
import { createServer } from "vite";
import { RESET_LIST, tagStore } from "./ad-tags-mock.mjs";

const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://localhost/" });
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createRoot } = await import("react-dom/client");
let server, App, root, container;

before(async () => {
  server = await createServer({
    root: fileURLToPath(new URL("../../", import.meta.url)),
    configFile: false, logLevel: "silent", appType: "custom",
    server: { middlewareMode: true, hmr: false, watch: null },
  });
  ({ default: App } = await server.ssrLoadModule("/src/App.tsx"));
});

afterEach(async () => {
  if (root) await act(async () => root.unmount());
  root = undefined;
  container?.remove();
  clearMocks();
});

after(async () => {
  await server?.close();
  dom.window.close();
  delete globalThis.window;
  delete globalThis.document;
  delete globalThis.IS_REACT_ACT_ENVIRONMENT;
});

async function mount(handler) {
  mockIPC((command, args) => {
    if (command === "queue_runner_state") return false;
    if (command === "settings_download_folder") return { path: "/Users/me/Downloads", error: null };
    if (command === "settings_format") return "default";
    if (command === "queue_storage_problem") return null;
    return handler(command, args);
  }, { shouldMockEvents: true });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => root.render(React.createElement(App)));
}

const START = ["Official Video", "4K", "MV"];

async function openSettings() {
  await act(async () => container.querySelector('button[aria-label="Settings"]').click());
}
async function closeSettings() {
  await act(async () => container.querySelector('button[aria-label="Close settings"]').click());
}
const editor = () => container.querySelector('[data-role="ad-tags"]');
const phrases = () => [...editor().querySelectorAll('input[aria-label^="Phrase "]')].map((i) => i.value);
const input = (label) => editor().querySelector(`input[aria-label="${label}"]`);
const removeButton = (phrase) => editor().querySelector(`button[aria-label="Remove ${phrase}"]`);
async function type(el, value) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set.call(el, value);
    el.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
  });
}
const focusOut = (el, to = null) =>
  act(async () => el.dispatchEvent(new dom.window.FocusEvent("focusout", { bubbles: true, relatedTarget: to })));
const enter = (el) =>
  act(async () => el.dispatchEvent(new dom.window.KeyboardEvent("keydown", { key: "Enter", bubbles: true })));
function deferred() {
  let resolve;
  const promise = new Promise((yes) => (resolve = yes));
  return { promise, resolve };
}

// The rest of the app's commands, answered so Settings opens cleanly.
function app(store, hold) {
  return (command, args) => {
    if (command === "queue_list") return [];
    if (command === "settings_naming_template") return "title";
    if (command === "naming_previews") return [];
    if (command === "settings_naming_provider") return "off";
    if (command === "naming_status") return { provider: "off" };
    const answer = () => store.apply(command, args);
    return hold?.(command) ? hold(command).then(answer) : answer();
  };
}

test("N9: Settings lists the saved phrases, collapsed under a count", async () => {
  const store = tagStore(START);
  await mount(app(store));
  await openSettings();
  assert.equal(editor().open, false, "collapsed by default");
  assert.match(editor().querySelector("summary").textContent, /\(3\)$/);
  assert.deepEqual(phrases(), START);
  assert.deepEqual(store.calls, [], "opening saves nothing");
});

test("N9: adding a phrase sends it alone and shows the list the backend saved", async () => {
  const store = tagStore(START);
  await mount(app(store));
  await openSettings();
  await type(input("New phrase"), "  (Lyrics) ");
  await act(async () => editor().querySelector("form").requestSubmit());
  assert.deepEqual(store.calls, [{ command: "settings_add_ad_tag", args: { phrase: "(Lyrics)" } }]);
  assert.deepEqual(phrases(), [...START, "Lyrics"]);
  assert.equal(input("New phrase").value, "", "the field clears once saved");
  await act(async () => editor().querySelector("form").requestSubmit());
  assert.equal(store.calls.length, 1, "an empty phrase isn't sent");
});

test("N9: an edit saves on Enter or ✓, never on leaving the field; an unchanged one saves nothing", async () => {
  const store = tagStore(START);
  await mount(app(store));
  await openSettings();
  assert.equal(editor().querySelector('[data-role="ad-tag-save"]'), null, "no ✓ without an edit");
  await type(input("Phrase 2"), "8K");
  assert.ok(editor().querySelector('button[aria-label="Save 4K"]'), "✓ shows while the text differs");
  await focusOut(input("Phrase 2"));
  assert.deepEqual(store.calls, [], "leaving the field saves nothing");
  assert.equal(input("Phrase 2").value, "8K", "and keeps the text");
  await enter(input("Phrase 2"));
  assert.deepEqual(store.calls.at(-1), { command: "settings_edit_ad_tag", args: { old: "4K", new: "8K" } });
  assert.deepEqual(phrases(), ["Official Video", "8K", "MV"]);
  await type(input("Phrase 3"), "Clip");
  await act(async () => editor().querySelector('button[aria-label="Save MV"]').click());
  assert.deepEqual(phrases(), ["Official Video", "8K", "Clip"], "✓ saves");
  await type(input("Phrase 3"), "Clip ");
  await enter(input("Phrase 3"));
  assert.equal(store.calls.length, 2, "the same text after trimming saves nothing");
  await type(input("Phrase 2"), "   ");
  await enter(input("Phrase 2"));
  assert.deepEqual(phrases(), ["Official Video", "Clip"], "an edit cleared to nothing removes it");
});

test("N9: removing every phrase is kept, and reset brings the starter list back", async () => {
  const store = tagStore(["MV"]);
  await mount(app(store));
  await openSettings();
  await act(async () => removeButton("MV").click());
  assert.deepEqual(store.calls.at(-1), { command: "settings_remove_ad_tag", args: { phrase: "MV" } });
  assert.deepEqual(phrases(), []);
  assert.ok(editor().querySelector('[data-role="ad-tags-empty"]'));
  await act(async () => editor().querySelector('[data-role="ad-tags-reset"]').click());
  assert.deepEqual(store.calls.at(-1).command, "settings_reset_ad_tags");
  assert.deepEqual(phrases(), RESET_LIST);
});

test("N9: a failed save keeps the list as it was and shows the reason literally", async () => {
  const store = tagStore(START);
  await mount((command, args) => {
    if (command === "settings_remove_ad_tag") throw "Couldn't save your settings: <disk full>";
    return app(store)(command, args);
  });
  await openSettings();
  await act(async () => removeButton("4K").click());
  assert.deepEqual(phrases(), START);
  const alert = editor().querySelector('[data-role="ad-tags-error"]');
  assert.equal(alert.textContent, "Couldn't save your settings: <disk full>");
  assert.equal(alert.querySelector("*"), null, "literal text, no markup");
});

// #104 review rounds 1 and 2: changes name their phrase and the backend applies them to the list
// as saved, one at a time here, so none lands on a neighbour or undoes another.

test("N9 review: while a change is saving the editor is disabled, so a second click does nothing", async () => {
  const gate = deferred();
  const store = tagStore(["MV", "4K", "Audio"]);
  await mount(app(store, (c) => (c === "settings_remove_ad_tag" && store.calls.length === 0 ? gate.promise : null)));
  await openSettings();
  await act(async () => removeButton("MV").click());
  assert.ok([...editor().querySelectorAll("input,button")].every((el) => el.disabled));
  await act(async () => removeButton("4K").click());
  await act(async () => gate.resolve());
  assert.deepEqual(store.tags(), ["4K", "Audio"], "only the phrase clicked first");
  await act(async () => removeButton("4K").click());
  assert.deepEqual(store.tags(), ["Audio"]);
});

test("N9 review: clearing a phrase, then clicking its ✕, removes it once and keeps its neighbours", async () => {
  const store = tagStore(["MV", "4K", "Audio"]);
  await mount(app(store));
  await openSettings();
  await type(input("Phrase 1"), "");
  await focusOut(input("Phrase 1"), removeButton("MV"));
  await act(async () => removeButton("MV").click());
  assert.deepEqual(store.tags(), ["4K", "Audio"]);
  assert.deepEqual(store.calls, [{ command: "settings_remove_ad_tag", args: { phrase: "MV" } }]);
});

test("N9 review: an edit the backend tidies, then that row's ✕, removes it and keeps its neighbours", async () => {
  const store = tagStore(["MV", "4K", "Audio"]);
  await mount(app(store));
  await openSettings();
  await type(input("Phrase 1"), "((Clip))");
  await enter(input("Phrase 1"));
  assert.deepEqual(phrases(), ["(Clip)", "4K", "Audio"]);
  await act(async () => removeButton("(Clip)").click());
  assert.deepEqual(store.tags(), ["4K", "Audio"]);
});

test("N9 review: closing Settings mid-save and removing more never brings a removed phrase back", async () => {
  const gates = [deferred(), deferred()];
  let held = 0;
  const store = tagStore(["MV", "4K", "Audio"]);
  const hold = (c) => (c === "settings_remove_ad_tag" && held < 2 ? gates[held++].promise : null);
  await mount(app(store, hold));
  await openSettings();
  await act(async () => removeButton("MV").click());
  await closeSettings();
  await openSettings();
  await act(async () => removeButton("4K").click());
  await closeSettings();
  await openSettings();
  await act(async () => gates[0].resolve());
  await act(async () => gates[1].resolve());
  await act(async () => removeButton("Audio").click());
  assert.deepEqual(store.tags(), []);
  await closeSettings();
  await openSettings();
  assert.deepEqual(phrases(), []);
});

test("N9 review: a reply that arrives after Settings reopened changes nothing shown", async () => {
  const gate = deferred();
  const store = tagStore(["MV", "4K"]);
  let first = true;
  await mount((command, args) => {
    if (command === "settings_remove_ad_tag" && first) {
      first = false;
      const answer = store.apply(command, args); // saved at once, answered late
      return gate.promise.then(() => answer);
    }
    return app(store)(command, args);
  });
  await openSettings();
  await act(async () => removeButton("MV").click());
  await closeSettings();
  await openSettings();
  await act(async () => removeButton("4K").click());
  assert.deepEqual(phrases(), []);
  await act(async () => gate.resolve());
  assert.deepEqual(phrases(), [], "the older reply is ignored");
});

test("N9 review: a refused edit's text survives an unrelated removal", async () => {
  const store = tagStore(["MV", "4K"]);
  await mount((command, args) => {
    if (command === "settings_edit_ad_tag") throw "read-only settings";
    return app(store)(command, args);
  });
  await openSettings();
  await type(input("Phrase 1"), "Clip");
  await enter(input("Phrase 1"));
  assert.equal(input("Phrase 1").value, "Clip");
  await act(async () => removeButton("4K").click());
  assert.deepEqual(store.tags(), ["MV"]);
  assert.equal(input("Phrase 1").value, "Clip", "still there to retry");
});

// #104 review round 3.

test("N9 review: a stored bracketed phrase is removed by its own ✕ and a plain one beside it stays", async () => {
  const store = tagStore(["Clip"]);
  await mount(app(store));
  await openSettings();
  await type(input("New phrase"), "((Clip))");
  await act(async () => editor().querySelector("form").requestSubmit());
  assert.deepEqual(phrases(), ["Clip", "(Clip)"]);
  await act(async () => removeButton("(Clip)").click());
  assert.deepEqual(store.tags(), ["Clip"]);
  assert.deepEqual(store.calls.at(-1), { command: "settings_remove_ad_tag", args: { phrase: "(Clip)" } });
});

test("N9 review: ✕ after an edit sends one removal and no edit, whatever focus does", async () => {
  const gate = deferred();
  const store = tagStore(["MV", "4K"]);
  await mount(app(store, (c) => (c === "settings_remove_ad_tag" ? gate.promise : null)));
  await openSettings();
  await type(input("Phrase 1"), "Clip");
  // WebKit: a clicked button isn't focused, so the field loses focus to nothing.
  await act(async () => removeButton("MV").dispatchEvent(new dom.window.MouseEvent("mousedown", { bubbles: true, cancelable: true })));
  await focusOut(input("Phrase 1"), null);
  await act(async () => removeButton("MV").click());
  await act(async () => gate.resolve());
  assert.deepEqual(store.calls, [{ command: "settings_remove_ad_tag", args: { phrase: "MV" } }], "one removal, no edit");
  assert.deepEqual(store.tags(), ["4K"]);
});

test("N9 review: leaving an edited field to Add or nowhere sends nothing and keeps the edit", async () => {
  const store = tagStore(["MV", "4K"]);
  await mount(app(store));
  await openSettings();
  await type(input("Phrase 1"), "Clip");
  await focusOut(input("Phrase 1"), input("New phrase"));
  await focusOut(input("Phrase 1"), null);
  assert.deepEqual(store.calls, []);
  assert.equal(input("Phrase 1").value, "Clip");
  assert.ok(editor().querySelector('button[aria-label="Save MV"]'));
});

test("N9 review: tabbing from an edited field to its ✕ and on sends nothing and keeps the edit", async () => {
  const store = tagStore(["MV", "4K"]);
  await mount(app(store));
  await openSettings();
  await type(input("Phrase 1"), "Clip");
  await focusOut(input("Phrase 1"), removeButton("MV"));
  await act(async () =>
    removeButton("MV").dispatchEvent(new dom.window.FocusEvent("focusout", { bubbles: true, relatedTarget: input("Phrase 2") })),
  );
  assert.deepEqual(store.calls, []);
  assert.equal(input("Phrase 1").value, "Clip");
});

// #104 review round 4 and the architect's ruling (D-028): no change starts from focus moving;
// the window keeps the newest list; drafts are a Map; Enter ignores input-method composition.

test("N9 review: Reset after an edit, in browser event order with the reply held, resets and drops the edit", async () => {
  const gate = deferred();
  const store = tagStore(["MV", "4K"]);
  await mount(app(store, (c) => (c === "settings_reset_ad_tags" ? gate.promise : null)));
  await openSettings();
  await type(input("Phrase 1"), "Clip");
  const resetButton = editor().querySelector('[data-role="ad-tags-reset"]');
  await act(async () => resetButton.dispatchEvent(new dom.window.MouseEvent("mousedown", { bubbles: true, cancelable: true })));
  await focusOut(input("Phrase 1"), null);
  await act(async () => resetButton.click());
  await act(async () => gate.resolve());
  assert.deepEqual(store.calls.map((c) => c.command), ["settings_reset_ad_tags"]);
  assert.deepEqual(phrases(), RESET_LIST, "no edit left over");
});

test("N9 review: Add after an edit sends one add and keeps the edit", async () => {
  const gate = deferred();
  const store = tagStore(["MV", "4K"]);
  await mount(app(store, (c) => (c === "settings_add_ad_tag" ? gate.promise : null)));
  await openSettings();
  await type(input("Phrase 1"), "Clip");
  await type(input("New phrase"), "8K");
  await focusOut(input("Phrase 1"), input("New phrase"));
  await act(async () => editor().querySelector("form").requestSubmit());
  await act(async () => gate.resolve());
  assert.deepEqual(store.calls, [{ command: "settings_add_ad_tag", args: { phrase: "8K" } }]);
  assert.deepEqual(phrases(), ["Clip", "4K", "8K"], "the edit is still in its field, unsaved");
  assert.deepEqual(store.tags(), ["MV", "4K", "8K"]);
});

test("N9 review: Enter while an input method is composing saves nothing", async () => {
  const store = tagStore(["MV"]);
  await mount(app(store));
  await openSettings();
  await type(input("Phrase 1"), "広告");
  for (const init of [{ key: "Enter", isComposing: true }, { key: "Enter", keyCode: 229 }]) {
    await act(async () => input("Phrase 1").dispatchEvent(new dom.window.KeyboardEvent("keydown", { bubbles: true, ...init })));
  }
  assert.deepEqual(store.calls, []);
  await enter(input("Phrase 1"));
  assert.deepEqual(store.tags(), ["広告"]);
});

for (const order of ["reply before reopening", "reply after reopening"]) {
  test(`N9 review: a change pending when Settings closes shows when reopened (${order})`, async () => {
    const gate = deferred();
    const store = tagStore(["MV", "4K"]);
    await mount(app(store, (c) => (c === "settings_remove_ad_tag" ? gate.promise : null)));
    await openSettings();
    await act(async () => removeButton("MV").click());
    await closeSettings();
    if (order === "reply before reopening") await act(async () => gate.resolve());
    await openSettings();
    if (order === "reply after reopening") await act(async () => gate.resolve());
    assert.deepEqual(phrases(), ["4K"], "no interaction needed");
  });
}

test("N9 review: phrases named like object properties work like any other", async () => {
  const names = ["constructor", "__proto__", "toString", "hasOwnProperty"];
  const store = tagStore(names);
  await mount(app(store));
  await openSettings();
  assert.deepEqual(phrases(), names, "shown");
  await type(input("Phrase 1"), "constructors");
  assert.ok(editor().querySelector('button[aria-label="Save constructor"]'));
  await enter(input("Phrase 1"));
  await act(async () => removeButton("__proto__").click());
  await type(input("New phrase"), "__proto__");
  await act(async () => editor().querySelector("form").requestSubmit());
  assert.deepEqual(store.tags(), ["constructors", "toString", "hasOwnProperty", "__proto__"]);
  await closeSettings();
  await openSettings();
  assert.deepEqual(phrases(), ["constructors", "toString", "hasOwnProperty", "__proto__"], "shown again after reopening");
  assert.equal(editor().querySelector('[data-role="ad-tag-save"]'), null, "no stray edits");
});
