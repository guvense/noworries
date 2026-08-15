//! Golden-file (snapshot) assertion on the check's HTTP `request` response body.
//! Compares the JSON-normalized response to a saved file; `--update-snapshots`
//! writes/refreshes the golden instead of failing.

use std::time::Duration;

use serde_json::Value as Json;

use super::{fail, pass, Assertion};
use crate::runner::{AssertionResult, RunnerContext};
use crate::spec::CheckSpec;

pub struct Snapshot;

impl Assertion for Snapshot {
    fn run(&self, check: &CheckSpec, ctx: &RunnerContext, _retry: Duration) -> Vec<AssertionResult> {
        let Some(s) = &check.snapshot else { return vec![] };

        let Some(body) = ctx.http_capture.lock().ok().and_then(|c| c.clone()) else {
            return vec![fail(
                "snapshot",
                "snapshot needs the check's `request` response (add a request)".into(),
                None,
                None,
            )];
        };

        // Normalize to JSON where possible (stable comparison); fall back to text.
        let actual = normalize(&body, &s.ignore);
        let path = ctx.dir.join(&s.file);

        if ctx.update_snapshots {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let pretty = serde_json::to_string_pretty(&actual).unwrap_or_default();
            return match std::fs::write(&path, format!("{pretty}\n")) {
                Ok(()) => vec![pass("snapshot", format!("updated golden {}", s.file))],
                Err(e) => vec![fail("snapshot", format!("could not write {}: {e}", s.file), None, None)],
            };
        }

        let Ok(golden_text) = std::fs::read_to_string(&path) else {
            return vec![fail(
                "snapshot",
                format!("golden {} missing — run with --update-snapshots to create it", s.file),
                None,
                None,
            )];
        };
        let golden = normalize(&golden_text, &s.ignore);

        if golden == actual {
            vec![pass("snapshot", format!("matches golden {}", s.file))]
        } else {
            vec![fail(
                "snapshot",
                format!("differs from golden {} (--update-snapshots to accept)", s.file),
                Some(golden),
                Some(actual),
            )]
        }
    }
}

/// Parse as JSON (blanking `ignore` paths) or keep the raw string.
fn normalize(text: &str, ignore: &[String]) -> Json {
    match serde_json::from_str::<Json>(text) {
        Ok(mut v) => {
            for path in ignore {
                blank_path(&mut v, path);
            }
            v
        }
        Err(_) => Json::String(text.trim().to_string()),
    }
}

/// Set the value at a `$.a.b` / `a.b` path to null (so volatile fields don't
/// cause spurious diffs). Missing paths are ignored.
fn blank_path(v: &mut Json, path: &str) {
    let parts: Vec<&str> = path.trim_start_matches("$.").trim_start_matches('$').split('.').filter(|s| !s.is_empty()).collect();
    blank_rec(v, &parts);
}

fn blank_rec(v: &mut Json, parts: &[&str]) {
    let Some((head, rest)) = parts.split_first() else {
        *v = Json::Null;
        return;
    };
    match v {
        Json::Object(map) => {
            if let Some(child) = map.get_mut(*head) {
                blank_rec(child, rest);
            }
        }
        Json::Array(arr) => {
            if let Ok(i) = head.parse::<usize>() {
                if let Some(child) = arr.get_mut(i) {
                    blank_rec(child, rest);
                }
            } else {
                // apply to every element (e.g. `items.id`)
                for child in arr.iter_mut() {
                    blank_rec(child, parts);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_blanks_ignored_paths() {
        let v = normalize(r#"{"id": 7, "status": "OK", "items":[{"id":1},{"id":2}]}"#, &["$.id".into(), "items.id".into()]);
        assert_eq!(
            v,
            json!({"id": null, "status": "OK", "items":[{"id":null},{"id":null}]})
        );
    }

    #[test]
    fn non_json_falls_back_to_text() {
        assert_eq!(normalize("hello\n", &[]), json!("hello"));
    }
}
