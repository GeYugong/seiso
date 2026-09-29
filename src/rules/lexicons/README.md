---
kind: reference
---

# Built-in phrase lexicons

[normative.toml](normative.toml) and [heuristic.toml](heuristic.toml) are
embedded with `include_str!`. They contain the language-specific trigger and
constraint entries used by the convention and heuristic sentence rules.
Generic syntactic checks, numeric patterns, section classification, and
evidence detection remain in their owning modules.

Each `[[group.language]]` entry declares a `phrase`, optional `guard`, and
nonempty `hits` and `misses` arrays. Languages are `en`, `zh`, and `ja`.
Examples contain `text` (Markdown) and an optional `target` for rules that
report a value or link instead of the phrase. Examples run as `howto`
documents with an explicit language, preventing language detection from
silently testing a different lexicon.

| Group | Owning rule | Diagnostic location |
| --- | --- | --- |
| stale | STL001 | volatile value, named by `target` |
| constraint | STL001, also used by STL004 | suppresses a value diagnostic |
| commit | STL003 | hash, named by `target` |
| pointer | PTR001 | repository link, named by `target` |
| source | PTR003 | phrase |
| rationale | RAT002 | heading phrase |
| conversation | VOX001 | phrase |
| deployment | STL002 | phrase |
| excluded_heading | VOX002 | heading phrase |
| production_heading, narration | VOX003 | phrase |
| evaluation | EVD001 | phrase |

`guard = "end-user"` rejects negation, condition, requirement, or verification
context in the phrase's own clause, and attributive or temporal suffixes.
It is optional for any entry. Conversation entries and configured
conversation additions retain the preexisting guard; other additions remain
unguarded. Matching still uses ASCII case folding and word boundaries for
English, character matching for CJK, and the existing source span mappings.

The unit test in [lexicon.rs](../lexicon.rs) runs the owning rule on every
example. A positive diagnostic must overlap the tested phrase or explicit
target; a diagnostic elsewhere cannot satisfy the test. Each example runs
with the full lexicon and again with only its own entry in the tested group,
so an overlapping sibling cannot make a broken entry pass. Other groups keep
their usual entries for the rule's secondary checks. A thread-local test hook
isolates the entry and restores the selection after each run; release builds
do not contain that hook. Ordinary misses must produce no diagnostic.
Constraint entries are suppression cues: their hits must suppress a snapshot,
while their misses put the constraint in another sentence and require the
snapshot's value diagnostic. This checks the cue through the owning rule
without changing diagnostic locations to fit the test format.

Run all entry examples with:

```sh
cargo test --locked --lib every_builtin_phrase
```

The fixtures cover constrained values, specific source targets, body text
versus headings, end-user checks, quoted messages, and claims with evidence.
They are intended-use tuning examples, not an independent evaluation corpus.
All 208 existing entries are retained. No new phrase or guard behavior is
introduced by the migration. Existing integration tests retain the generated
and community-maintained ownership notices that motivated issue #12.

Before changing an entry, add a failing real-use example and its near miss.
Keep phrases and examples in the same change, and record removed or guarded
entries in the PR. A fresh holdout under the
[evaluation policy](../../../docs/evaluation/policy.md) is required before
claiming accuracy improvements or promoting a rule. Passing these fixtures
alone establishes regression coverage, not general language accuracy.
