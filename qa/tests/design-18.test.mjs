// #18 (D-045): native-minimal, enforced. Contrast is computed from the pinned Tailwind palette
// (node_modules/tailwindcss/theme.css), never retyped: every text colour against every text
// surface in its theme at 4.5:1, text-field borders and the progress fill at 3:1. Colour appears
// only as the exact red and amber status classes; nothing is dimmed with opacity or alpha except
// disabled controls; the OS's own focus ring is never hidden or restyled; motion stops under
// prefers-reduced-motion; the font is system-ui first with nothing bundled.
import assert from "node:assert/strict";
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { test } from "node:test";

const root = new URL("../../", import.meta.url);
const read = (p) => readFileSync(new URL(p, root), "utf8");

/** Every .tsx file under src/. */
function sources(dir = "src/") {
  const out = [];
  for (const entry of readdirSync(new URL(dir, root), { withFileTypes: true })) {
    if (entry.isDirectory()) out.push(...sources(`${dir}${entry.name}/`));
    else if (entry.name.endsWith(".tsx")) out.push({ file: `${dir}${entry.name}`, text: read(`${dir}${entry.name}`) });
  }
  return out;
}

// --- The palette, from the pinned theme.css -------------------------------------------------

const theme = read("node_modules/tailwindcss/theme.css");
function colour(name) {
  const m = theme.match(new RegExp(`--color-${name}:\\s*([^;]+);`));
  assert.ok(m, `--color-${name} isn't in theme.css`);
  return m[1].trim();
}

/** Linear-light sRGB channels of an oklch(...) or #hex value. */
function linear(value) {
  if (value.startsWith("#")) {
    const hex = value.length === 4 ? [...value.slice(1)].map((c) => c + c).join("") : value.slice(1);
    return [0, 2, 4].map((i) => {
      const c = parseInt(hex.slice(i, i + 2), 16) / 255;
      return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
  }
  const m = value.match(/oklch\(\s*([\d.]+)%\s+([\d.]+)\s+([\d.]+|none)\s*\)/);
  assert.ok(m, `can't read ${value}`);
  const L = Number(m[1]) / 100;
  const C = Number(m[2]);
  const h = m[3] === "none" ? 0 : (Number(m[3]) * Math.PI) / 180;
  const a = C * Math.cos(h);
  const b = C * Math.sin(h);
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const mm = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const clamp = (x) => Math.min(1, Math.max(0, x));
  return [
    clamp(4.0767416621 * l - 3.3077115913 * mm + 0.2309699292 * s),
    clamp(-1.2684380046 * l + 2.6097574011 * mm - 0.3413193965 * s),
    clamp(-0.0041960863 * l - 0.7034186147 * mm + 1.707614701 * s),
  ];
}
const luminance = (name) => {
  const [r, g, b] = linear(colour(name));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
export function contrast(fg, bg) {
  const [hi, lo] = [luminance(fg), luminance(bg)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

// --- D-045's table -------------------------------------------------------------------------

const TEXT = {
  light: ["neutral-900", "neutral-800", "neutral-700", "neutral-600", "red-700", "amber-800"],
  dark: ["neutral-100", "neutral-200", "neutral-300", "neutral-400", "red-400", "amber-400"],
};
const SURFACES = { light: ["white", "neutral-100", "neutral-200"], dark: ["neutral-900", "neutral-800"] };
/** The inverted primary button, the toast and the progress fill: their own pair, not text surfaces. */
const INVERTED = { light: { bg: "neutral-900", text: "white" }, dark: { bg: "neutral-100", text: "neutral-900" } };
const FIELD_BORDER = "neutral-500";
/** Non-text colours allowed as they are: dividers, secondary-button borders, the spinner. */
const BORDERS = { light: ["neutral-200", "neutral-300", "neutral-500", "neutral-600", "transparent"], dark: ["neutral-500", "neutral-700", "neutral-800", "neutral-400", "transparent"] };
const STATUS_CLASSES = new Set(["text-red-700", "dark:text-red-400", "text-amber-800", "dark:text-amber-400"]);

const COLOUR = /(?<![\w-])((?:[a-z-]+:)*)(text|bg|border(?:-[trblxy])?|divide|ring|outline|placeholder|fill|stroke|accent|caret|decoration|from|to|via|shadow)-(white|black|transparent|current|inherit|[a-z]+-\d{2,3})(\/\d+)?(?![\w-])/g;

/** Every colour utility in src/, with its variants. */
function colourUses() {
  const uses = [];
  for (const { file, text } of sources()) {
    for (const m of text.matchAll(COLOUR)) {
      const variants = m[1].split(":").filter(Boolean);
      uses.push({ file, token: m[0], variants, kind: m[2], name: m[3], alpha: m[4] ?? null, dark: variants.includes("dark") });
    }
  }
  return uses;
}

test("#18: every text colour reaches 4.5:1 on every text surface in its theme", () => {
  for (const scheme of ["light", "dark"]) {
    for (const fg of TEXT[scheme]) {
      for (const bg of SURFACES[scheme]) {
        const ratio = contrast(fg, bg);
        assert.ok(ratio >= 4.5, `${scheme}: ${fg} on ${bg} is ${ratio.toFixed(2)}:1`);
      }
    }
    const { bg, text } = INVERTED[scheme];
    assert.ok(contrast(text, bg) >= 4.5, `${scheme}: the inverted pair ${text} on ${bg}`);
  }
});

test("#18: text-field borders and the progress fill reach 3:1", () => {
  for (const bg of [...SURFACES.light, ...SURFACES.dark]) {
    assert.ok(contrast(FIELD_BORDER, bg) >= 3, `the field border on ${bg} is ${contrast(FIELD_BORDER, bg).toFixed(2)}:1`);
  }
  // The progress bar (QueueList): neutral-900 on a neutral-200 track; neutral-100 on neutral-800.
  assert.ok(contrast("neutral-900", "neutral-200") >= 3);
  assert.ok(contrast("neutral-100", "neutral-800") >= 3);
  const queue = read("src/components/QueueList.tsx");
  assert.match(queue, /bg-neutral-200 dark:bg-neutral-800"\s*>\s*<div className="h-full bg-neutral-900[^"]*dark:bg-neutral-100"/, "the progress track and fill are the pairs checked");
  // Exempt, by design: dividers, secondary-button borders, the track against the page, native
  // controls and disabled controls.
});

test("#18: every colour class in src/ is one D-045's table knows", () => {
  const problems = [];
  for (const u of colourUses()) {
    const scheme = u.dark ? "dark" : "light";
    const hue = u.name.match(/^([a-z]+)-\d/)?.[1];
    const where = `${u.file}: ${u.token}`;
    if (u.alpha) problems.push(`${where}: no alpha colours`);
    if (hue && hue !== "neutral") {
      // Colour is status only, and only these exact classes.
      if (!STATUS_CLASSES.has(u.token)) problems.push(`${where}: no hue other than the exact red and amber status classes`);
      continue;
    }
    if (u.kind === "text" || u.kind === "placeholder") {
      const known = [...TEXT[scheme], INVERTED[scheme].text];
      if (!known.includes(u.name)) problems.push(`${where}: not a ${scheme} text colour`);
    } else if (u.kind === "bg") {
      const known = [...SURFACES[scheme], INVERTED[scheme].bg, "transparent"];
      if (!known.includes(u.name)) problems.push(`${where}: not a ${scheme} surface`);
    } else if (u.kind.startsWith("border") || u.kind === "divide") {
      if (!BORDERS[scheme].includes(u.name)) problems.push(`${where}: not a ${scheme} border`);
    } else {
      problems.push(`${where}: ${u.kind}-* colours aren't used`);
    }
  }
  assert.deepEqual(problems, []);
});

test("#18: no text is dimmed with opacity, and the OS's focus ring is never hidden or restyled", () => {
  const problems = [];
  for (const { file, text } of sources()) {
    for (const m of text.matchAll(/(?<![\w-])((?:[a-z-]+:)*)opacity-\d+/g)) {
      if (!m[1].split(":").includes("disabled")) problems.push(`${file}: ${m[0]} (only disabled:opacity-*)`);
    }
    for (const m of text.matchAll(/(?<![\w-])(?:[a-z-]+:)*(outline-none|outline-hidden|outline-0|ring-\S+|accent-\S+)/g)) {
      problems.push(`${file}: ${m[0]}`);
    }
    for (const m of text.matchAll(/(?<![\w-])focus(?:-visible|-within)?:(?:border|bg|text|outline|ring|shadow)-\S+/g)) {
      problems.push(`${file}: ${m[0]}`);
    }
  }
  // The scroll area clips a row's ring unless the list keeps room on every side (D-045).
  const queue = read("src/components/QueueList.tsx");
  assert.match(queue, /<ul aria-label="Queue" className="[^"]*\bpx-1\b[^"]*\bpb-1\b/, "the list keeps room for the ring at its sides and bottom");
  assert.match(queue, /className=\{`flex scroll-my-1\b/, "a focused row scrolls into view with room for its ring");
  const css = read("src/index.css").replace(/\/\*[\s\S]*?\*\//g, "");
  assert.doesNotMatch(css, /accent-color|outline:\s*(none|0)/, "index.css sets no accent-color and hides no outline");
  assert.deepEqual(problems, []);
});

test("#18: every text field has a 3:1 border and an explicit placeholder colour", () => {
  const fields = [];
  for (const { file, text } of sources()) {
    // An element's attributes run to its `/>` (input) or its closing tag (select, textarea):
    // attribute values hold arrow functions, so the first ">" isn't the end.
    for (const m of text.matchAll(/<(input|textarea|select)\b([\s\S]*?)(?:\/>|<\/(?:select|textarea)>)/g)) {
      const attrs = m[2];
      if (/type="(radio|checkbox)"/.test(attrs)) continue;
      fields.push({ file, attrs, tag: m[1] });
    }
  }
  assert.ok(fields.length >= 6, `found ${fields.length} fields`);
  for (const { file, attrs, tag } of fields) {
    // A field's class may be a shared constant (ApiSetup's `field`); resolve it.
    const cls = attrs.match(/className=(?:"([^"]*)"|\{(\w+)\})/);
    assert.ok(cls, `${file}: <${tag}> has no className`);
    const value = cls[1] ?? read(file).match(new RegExp(`const ${cls[2]} =\\s*"([^"]*)"`))?.[1];
    assert.ok(value, `${file}: can't resolve ${cls[2]}`);
    if (/\bborder\b/.test(value)) {
      assert.match(value, /(?<![\w:-])border-neutral-500\b/, `${file}: <${tag}> border must be ${FIELD_BORDER}`);
      assert.match(value, /dark:border-neutral-500\b/, `${file}: <${tag}> dark border must be ${FIELD_BORDER}`);
    }
    if (/placeholder=/.test(attrs) || tag === "input") {
      if (/placeholder=/.test(attrs)) {
        assert.match(value, /placeholder:text-neutral-600/, `${file}: <${tag}> needs an explicit placeholder colour`);
        assert.match(value, /dark:placeholder:text-neutral-400/, `${file}: <${tag}> needs an explicit dark placeholder colour`);
      }
    }
  }
});

test("#18: animation is the spinner and two transitions, all stopped under reduced motion", () => {
  const animated = [];
  for (const { file, text } of sources()) {
    for (const m of text.matchAll(/className="([^"]*)"|className=\{`([^`]*)`\}/g)) {
      const cls = m[1] ?? m[2];
      const moves = cls.match(/(?<![\w:-])(animate-\w+|transition(?:-[\w[\]]+)?)/g);
      if (!moves) continue;
      animated.push(`${file}: ${moves.join(" ")}`);
      if (moves.some((x) => x.startsWith("animate-"))) assert.match(cls, /motion-reduce:animate-none/, `${file}: ${moves}`);
      if (moves.some((x) => x.startsWith("transition"))) assert.match(cls, /motion-reduce:transition-none/, `${file}: ${moves}`);
    }
  }
  assert.equal(animated.length, 3, animated.join("\n"));
  assert.ok(animated.some((a) => a.includes("animate-spin")), "the spinner");
});

test("#18: the spinner sits on Fetching and Processing rows, hidden from screen readers", () => {
  const queue = read("src/components/QueueList.tsx");
  assert.match(queue, /function Spinner\(\)[\s\S]*?aria-hidden="true"/);
  assert.match(queue, /<Spinner \/>\s*Fetching info…/);
  assert.match(queue, /<Spinner \/>\s*Processing…/);
  assert.equal(queue.match(/<Spinner \/>/g)?.length, 2, "nowhere else");
});

test("#18: system-ui first through --font-sans, nothing bundled, and color-scheme for OS-drawn controls", () => {
  const css = read("src/index.css");
  assert.match(css, /@theme\s*\{\s*--font-sans:\s*system-ui,/);
  assert.doesNotMatch(css, /--font-mono|@font-face|font-family\s*:/, "only --font-sans is overridden, through @theme");
  assert.match(css, /Noto Color Emoji/, "Tailwind's emoji fallbacks are kept");
  assert.match(css, /:root\s*\{\s*color-scheme:\s*light dark;\s*\}/);
  const assets = new URL("dist/assets/", root);
  if (existsSync(assets)) {
    const built = readdirSync(assets).filter((f) => f.endsWith(".css")).map((f) => read(`dist/assets/${f}`)).join("\n");
    assert.match(built, /--font-sans:\s*system-ui/, "the build starts the font stack with system-ui");
    assert.doesNotMatch(built, /@font-face/);
  }
});

test("#18: no colour-emoji glyph is used as an icon; the settings gear is drawn in currentColor", () => {
  const app = read("src/App.tsx");
  assert.match(app, /data-role="settings-icon"[^>]*stroke="currentColor"/);
  for (const { file, text } of sources()) {
    const code = text.replace(/\/\*[\s\S]*?\*\/|\/\/.*$/gm, "");
    assert.doesNotMatch(code, /⚙/, `${file}: the gear renders as a colour emoji in WebView2`);
  }
});
