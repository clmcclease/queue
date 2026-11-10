// A stand-in for #104's tag-list commands (src-tauri/src/lib.rs `settings_*ad_tag*`), shared by
// the editor's UI tests. Like the backend, each change names its phrase and applies to the list
// as saved, a missing phrase changes nothing, and every change bumps the version. Tidying is
// approximated: trimmed, single spaces, duplicates (ignoring case) and empty phrases dropped, and
// one surrounding () or [] dropped from typed text only.
export const RESET_LIST = ["Official Video", "Official Music Video"];

export function tagStore(initial) {
  let version = 0;
  let tags = [...initial];
  const calls = [];
  const collapse = (t) => t.trim().replace(/\s+/g, " ");
  // Only typed text loses a surrounding () or [] (as in the backend since review round 3);
  // stored phrases are found and kept exactly.
  const typed = (t) => {
    const s = collapse(t);
    const m = /^\((.*)\)$/.exec(s) || /^\[(.*)\]$/.exec(s);
    return m && m[1].trim() ? collapse(m[1]) : s;
  };
  const key = (t) => collapse(t).toLowerCase();
  const tidyAll = (list) => {
    const seen = new Set();
    return list.map(collapse).filter((t) => t && !seen.has(key(t)) && seen.add(key(t)));
  };
  const reply = () => ({ version, tags: [...tags] });
  /** Answers one command, or returns undefined for one that isn't a tag-list command. */
  function apply(command, args) {
    if (command === "settings_ad_tags") return reply();
    const at = (phrase) => tags.findIndex((t) => key(t) === key(phrase));
    switch (command) {
      case "settings_add_ad_tag":
        tags = tidyAll([...tags, typed(args.phrase)]);
        break;
      case "settings_edit_ad_tag": {
        const i = at(args.old);
        if (i >= 0) tags = tidyAll(tags.map((t, j) => (j === i ? typed(args.new) : t)));
        break;
      }
      case "settings_remove_ad_tag": {
        const i = at(args.phrase);
        if (i >= 0) tags = tags.filter((_, j) => j !== i);
        break;
      }
      case "settings_reset_ad_tags":
        tags = [...RESET_LIST];
        break;
      default:
        return undefined;
    }
    calls.push({ command, args });
    version += 1;
    return reply();
  }
  return { apply, calls, tags: () => tags, version: () => version };
}
