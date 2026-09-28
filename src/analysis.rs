//! Run enabled rules against a loaded snapshot and select the reported diagnostics.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::CliOverrides;
use crate::diagnostics::{Diagnostic, sorted_diagnostics};
use crate::rules::{self, CheckContext, RawCheckResult};
use crate::workspace::{InputError, Snapshot};

pub struct Analysis {
    pub snapshot: Snapshot,
    pub diagnostics: Vec<Diagnostic>,
}

impl Analysis {
    pub fn exit_code(&self, exit_zero: bool) -> u8 {
        if !self.snapshot.errors.is_empty() {
            2
        } else if !exit_zero && !self.diagnostics.is_empty() {
            1
        } else {
            0
        }
    }

    pub fn has_fixes(&self) -> bool {
        self.diagnostics.iter().any(|diagnostic| {
            diagnostic.fix.as_ref().is_some_and(|fix| {
                fix.applicability == crate::diagnostics::Applicability::Safe
                    && !fix.edits.is_empty()
            })
        })
    }

    pub fn same_inputs_and_diagnostics(&self, other: &Self) -> bool {
        self.diagnostics == other.diagnostics
            && self.snapshot.index.files().len() == other.snapshot.index.files().len()
            && self
                .snapshot
                .index
                .files()
                .iter()
                .zip(other.snapshot.index.files())
                .all(|(a, b)| a.filename == b.filename && a.document.source == b.document.source)
    }
}

/// Inspect declarations without claiming that their rules have executed.
pub fn inspect_policy(snapshot: &mut Snapshot) {
    for file in snapshot.index.files() {
        if file.kind.as_deref() == Some("generated") {
            continue;
        }
        let enabled = file.enabled_rules.iter().cloned().collect();
        if let Some(policy) = snapshot.policies.get_mut(&file.filename) {
            policy.suppressions = rules::suppression::inspect(&file.document, &enabled);
        }
    }
}

pub fn check(mut snapshot: Snapshot, overrides: &CliOverrides) -> Result<Analysis, String> {
    let index = &snapshot.index;
    let mut cross = if index.files().iter().any(|file| {
        file.enabled_rules
            .iter()
            .any(|code| rules::rule(code).is_some_and(|rule| rule.requires_index))
    }) {
        rules::cross_file::check(index)
    } else {
        rules::cross_file::CrossReport::default()
    };
    snapshot.errors.extend(
        cross
            .errors
            .into_iter()
            .map(|(filename, message)| InputError { filename, message }),
    );
    let mut cross_by_file = BTreeMap::<String, Vec<Diagnostic>>::new();
    for diagnostic in cross.diagnostics {
        cross_by_file
            .entry(diagnostic.filename.clone())
            .or_default()
            .push(diagnostic);
    }
    let mut diagnostics = Vec::new();
    for file in index.files() {
        let selected = snapshot.selected.contains(&file.filename);
        if !selected && !cross_by_file.contains_key(&file.filename) {
            continue;
        }
        let mut raw = if selected {
            rules::check_raw_with_files(
                &CheckContext {
                    document: &file.document,
                    filename: &file.filename,
                    path: &file.path,
                    workspace_root: &index.root,
                    config: &file.config,
                    overrides,
                },
                index,
            )
            .map_err(|error| error.to_string())?
        } else {
            RawCheckResult {
                kind: rules::resolve_kind(&file.document, file.config.kind_for(&file.path)),
                enabled_rules: BTreeSet::new(),
                diagnostics: Vec::new(),
                errors: Vec::new(),
                incomplete_rules: rules::single_file_rules()
                    .map(|rule| rule.code.to_owned())
                    .collect(),
            }
        };
        raw.enabled_rules = file.enabled_rules.iter().cloned().collect();
        raw.diagnostics
            .extend(cross_by_file.remove(&file.filename).unwrap_or_default());
        raw.incomplete_rules
            .extend(cross.incomplete.remove(&file.filename).unwrap_or_default());
        if !index.complete {
            raw.incomplete_rules
                .extend(rules::cross_file_rules().map(|rule| rule.code.to_owned()));
        }
        let result = rules::finish_check(&file.document, &file.filename, raw);
        snapshot
            .errors
            .extend(result.errors.into_iter().map(|message| InputError {
                filename: file.filename.clone(),
                message,
            }));
        diagnostics.extend(result.diagnostics.into_iter().filter(|diagnostic| {
            snapshot.selected.contains(&diagnostic.filename)
                || (rules::rule(&diagnostic.code).is_some_and(|rule| rule.requires_index)
                    && diagnostic.related.iter().any(|related| {
                        snapshot.selected.contains(&related.filename)
                            || (diagnostic.code == "PTR002"
                                && snapshot.requested.iter().any(|selected| {
                                    index.root.join(&related.filename).starts_with(selected)
                                }))
                    }))
        }));
        if let Some(policy) = snapshot.policies.get_mut(&file.filename) {
            policy.suppressions = result.suppressions;
        }
    }
    snapshot.sort_errors();
    Ok(Analysis {
        snapshot,
        diagnostics: sorted_diagnostics(&diagnostics),
    })
}
