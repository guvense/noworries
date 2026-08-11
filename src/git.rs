//! Changed-file detection so `/noworries` can scope to what the AI just
//! touched, and `/noworries force` (`changed --all`) can cover everything.
//!
//! The CLI only *reports* the file list; Claude reads it and decides which
//! checks in noworries.yml to add/run. This keeps the deterministic part
//! (which files changed) in the tool and the smart part (what to verify) in
//! the model.

use std::collections::BTreeSet;
use std::process::Command;

fn git_lines(dir: &str, args: &[&str]) -> Vec<String> {
    let mut full = vec!["-C", dir];
    full.extend_from_slice(args);
    match Command::new("git").args(&full).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// Files changed vs HEAD (modified + staged) plus untracked files.
pub fn changed_files(dir: &str) -> Vec<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    for f in git_lines(dir, &["diff", "--name-only", "HEAD"]) {
        set.insert(f);
    }
    for f in git_lines(dir, &["ls-files", "--others", "--exclude-standard"]) {
        set.insert(f);
    }
    set.into_iter().collect()
}

/// All tracked files (the "force"/regression scope).
pub fn all_tracked(dir: &str) -> Vec<String> {
    git_lines(dir, &["ls-files"])
}
