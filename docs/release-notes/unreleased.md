---
kind: changelog
---

# Unreleased

## Configuration inheritance

Extending an ancestor's governing configuration now retains the base directory
of its path entries. A parent exclusion such as `docs/generated/**` continues
to match when inherited by `docs/seiso.toml`. Kind, domain, and site mappings,
include patterns, per-file ignores, and catalog directories follow the same
rule. Shared templates keep their existing caller-relative behavior.

Configurations that relied on rebasing a governing parent's entries should
declare those entries locally or move the reusable policy to a separate
template. With `--config`, the selected configuration's own entries still
use the workspace root. Effective configurations in `seiso policy` now include
`pattern_bases` with each entry's workspace-relative base directory.
