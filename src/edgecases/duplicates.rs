//! `duplicates` — idempotency / exactly-once.
//!
//! Produces `count` distinct messages (default 50) and sends **each one twice**
//! (same key, same payload). A correct pipeline dedups, so the observable effect
//! matches the *unique* count, not double. Verify with
//! `db.expect_row_count: <count>` (not `2 * count`); a duplicate-processing bug
//! shows up as twice the rows / double-counted totals.

use anyhow::{bail, Result};

use super::{action_for, delay_from_rate, EdgeCase, ScenarioPlan};
use crate::spec::ScenarioSpec;

pub struct Duplicates;

impl EdgeCase for Duplicates {
    fn kind(&self) -> &'static str {
        "duplicates"
    }

    fn describe(&self) -> &'static str {
        "every message delivered twice — verifies idempotent / exactly-once handling"
    }

    fn plan(&self, s: &ScenarioSpec) -> Result<ScenarioPlan> {
        let count = s.count.unwrap_or(50);
        if count == 0 {
            bail!("scenario duplicates: count must be > 0");
        }
        let concurrency = s.concurrency.unwrap_or(1).max(1) as usize;

        // Each logical message is its own key and is emitted twice back-to-back.
        let mut actions = Vec::with_capacity(count as usize * 2);
        for seq in 0..count {
            let action = action_for(s, seq, 0, format!("k{seq}"));
            actions.push(action.clone());
            actions.push(action);
        }

        Ok(ScenarioPlan {
            actions,
            concurrency,
            per_message_delay_ms: delay_from_rate(s.rate_per_sec),
            summary: format!(
                "duplicates: {count} messages, each sent twice ({} produced)",
                count * 2
            ),
        })
    }
}
