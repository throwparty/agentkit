use std::collections::HashSet;
use std::path::Path;

use crate::vcs::{HistoryStep, Revision};

use super::Error;
use super::ops;

/// How many operations to inspect before reporting that the log is capped.
const MAX_OPERATIONS: usize = 10_000;

/// Read the commits the bookmark `branch` pointed at.
///
/// jj records the repository view after every operation. The bookmark is
/// resolved at each operation with `--at-op`; a run of equal values is one
/// step. The scan is newest first and never writes.
pub(super) fn read(root: &Path, branch: &str) -> Result<Vec<HistoryStep>, Error> {
    let template = r#"self.id() ++ "\t" ++ self.time().start().format("%s") ++ "\t" ++ self.description().first_line() ++ "\n""#;
    let log = ops::jj(
        root,
        &[
            "--ignore-working-copy",
            "op",
            "log",
            "--no-graph",
            "--limit",
            &MAX_OPERATIONS.to_string(),
            "-T",
            template,
        ],
    )?;

    let revset = format!("bookmarks({branch})");
    let mut run: Option<(String, i64, String)> = None;
    let mut steps = Vec::new();
    let mut seen = HashSet::new();

    let mut push = |value: (String, i64, String), steps: &mut Vec<HistoryStep>| {
        if !seen.insert(value.0.clone()) {
            return;
        }
        steps.push(HistoryStep {
            revision: Revision::new(value.0),
            time: value.1,
            description: value.2,
            synced: false,
        });
    };

    let mut operations = 0;
    for line in log.lines() {
        let Some((id, rest)) = line.split_once('\t') else {
            continue;
        };
        let Some((time, description)) = rest.split_once('\t') else {
            continue;
        };
        operations += 1;
        let time = time.parse::<i64>().unwrap_or(0);
        let value = ops::jj(
            root,
            &[
                "--at-op",
                id,
                "--ignore-working-copy",
                "log",
                "--no-graph",
                "-r",
                &revset,
                "-T",
                "commit_id ++ \"\\n\"",
            ],
        )?
        .lines()
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

        match value {
            None => {
                if let Some(value) = run.take() {
                    push(value, &mut steps);
                }
            }
            Some(value) if run.as_ref().map(|run| run.0.as_str()) != Some(value.as_str()) => {
                if let Some(value) = run.take() {
                    push(value, &mut steps);
                }
                run = Some((value, time, description.to_string()));
            }
            Some(_) => {
                if let Some(run) = run.as_mut() {
                    run.1 = time;
                    run.2 = description.to_string();
                }
            }
        }
    }
    if let Some(value) = run.take() {
        push(value, &mut steps);
    }

    if operations >= MAX_OPERATIONS {
        tracing::warn!(
            operations = operations,
            "the jj operation log scan reached its cap; older steps are not listed"
        );
    }

    Ok(steps)
}
