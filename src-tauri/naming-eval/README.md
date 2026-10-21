# Naming eval set (N9, #33)

`titles.json` holds 54 **author-written seed** cases: items shaped like yt-dlp metadata, each with the name a good model should give (`expected`) and other acceptable names (`accept`). They check the prompt, the answer validator and the runner. They aren't a sample of real downloads. The owner reviews the expected names.

#33(b) replaces or extends them with the 200–300 **real** items the 2026-10-11 note asks for (≥30% non-English), taken from real `-J` output and labelled by the owner (D-016, D-029). The runner takes a set of any size.

`tests/naming_eval_33.rs` checks the set's shape on every run, and scores a model only when `QUEUE_EVAL_ENDPOINT` is set (see that file). An answer that translates or romanizes a non-Latin title is rejected by design (D-016), so Queue keeps the Tier-1 name; the runner counts these separately.
