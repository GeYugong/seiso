---
kind: reference
---

# The seiso convention

seiso checks document responsibilities, fact ownership, repository links, and
unsupported claims. It does not identify AI authorship, check spelling or
formatting, or rewrite meaning. The convention is independent of any writing
skill or prompt. Its original motivation is recorded in the [design history](../design/history.md).

## Document responsibilities

Each document has one `kind`, declared in YAML frontmatter or assigned by a
[path mapping](configuration.md). Declaring a kind accepts its content contract;
the declaration does not prove that a statement is correct or current.

| Kind | Question answered | Contents | Content that belongs elsewhere |
| --- | --- | --- | --- |
| `readme` | What is this, and how do I start? | Purpose, quick start, documentation links | Field catalogs, architecture detail, current version snapshots |
| `howto` | How do I do this? | Steps, commands, expected results | Design arguments, complete option references |
| `reference` | What exists? | Definitions, tables, accepted values | Procedures, project history |
| `runbook` | How do I respond to an incident? | Inspection commands, recovery steps | Current deployment state and version snapshots |
| `adr` | Why was this design chosen? | Context, decision, tradeoffs, date | No additional kind-specific restriction |
| `plan` | What work comes next? | Proposed stages, dates, progress | Authoritative contracts for completed behavior |
| `changelog` | What happened? | Dated changes, versions, commits | No additional kind-specific restriction |
| `generated` | What did the generator produce? | Generator-owned content | A frontmatter declaration cannot grant this exemption |

`readme`, `howto`, `reference`, and `runbook` are long-lived documents.
`generated` can only be assigned in configuration. Generated documents are
exempt from rules but remain index sources, link targets, and possible owners
of duplicated facts. Invalid frontmatter does not fall back to a configured
kind. See [KND001](../rules/KND001.md) and [KND002](../rules/KND002.md) for executable examples.

Classify a product requirements document (PRD) or specification using the
responsibilities in the table above. Before implementation, a PRD or feature
specification uses `plan`. A maintained specification of behavior that must
hold now uses `reference`, including protocol and file-format specifications.
Record design decisions and tradeoffs in an `adr` and link to it from either
document. Once behavior ships, move its definitions from the plan to a
reference page and link to that page from the plan. These documents use the
existing kinds; `prd` and `spec` are not additional kinds.

## Facts and pointers

A fact has one authoritative home. Other pages link to that home instead of
repeating its definition. Long-lived pages point to the code or command that
answers changing questions such as the current version, deployment state, or
item count. A dated record may preserve the value observed at that time.

A pointer identifies the file, symbol, or section that supplies the promised
answer. A directory is an appropriate target when it is a catalog whose loader
or naming convention explains how to select an entry.

Duplicate and ownership checks compare documents within the same configured
domain and language. `lang` frontmatter takes precedence over script-based
language detection; Chinese and Japanese versions do not compete with English
versions merely because they name the same identifiers.

Ownership is evaluated per pair of duplicate blocks, without transitive
merging. The precedence is `canonical: true`, then generated, reference, ADR,
how-to, README, and other kinds. `canonical` applies to the whole file. Ties
remain unresolved; Git age or discovery order never decides ownership.
Rule-specific matching belongs to [DUP001](../rules/DUP001.md),
[DUP002](../rules/DUP002.md), [DUP003](../rules/DUP003.md),
[OWN001](../rules/OWN001.md), and [OWN002](../rules/OWN002.md).

## Evidence and exceptions

The strength of a diagnostic must not exceed its evidence. Consistency rules
state mechanically established facts. Convention rules identify a breach of
the content contract, without claiming that a version has actually expired.
Heuristic rules describe observed features and make conditional suggestions.
Semantic repairs remain the author's or agent's decision.

Exceptions name complete rule codes and give a reviewable reason. The syntax
and scope are defined by [SUP001](../rules/SUP001.md); completion and unused-code
semantics are defined by [SUP002](../rules/SUP002.md). A suppression cannot make an
incomplete check complete.

## Rule availability

Rules are enabled or disabled; there is no warning tier. Stable rules are
selected by default. Preview rules require explicit preview opt-in as well as
selection. The [evaluation policy](../evaluation/policy.md) governs promotion.
`seiso rule --all` lists implemented rules and their embedded explanations;
the registry in [`src/rules/mod.rs`](../../src/rules/mod.rs) owns rule availability,
status, kind applicability, and input requirements. Proposed rules belong to
the [roadmap](../design/roadmap.md), and their codes are not accepted configuration selectors.
