//! `concurrent` — race conditions on the same key.
//!
//! Produces `count` writes (default 50) spread over a *small* number of `keys`
//! (default 1) across `concurrency` parallel producers (default 4), so many
//! updates to the same key land at once. Each message carries its per-key
//! sequence in `${i}`, so the app can implement last-write-wins / versioning.
//! Verify the pipeline stays consistent: one row per key
//! (`db.expect_row_count: <keys>`) and/or the final value equals the highest
//! `${i}`. If the app has a race bug, you get duplicate rows or a stale value.

use anyhow::{bail, Result};

use super::{action_for, delay_from_rate, EdgeCase, ScenarioPlan};
use crate::spec::ScenarioSpec;

pub struct Concurrent;

impl EdgeCase for Concurrent {
    fn kind(&self) -> &'static str {
        "concurrent"
    }

    fn describe(&self) -> &'static str {
        "parallel writes to the same key(s) — verifies race handling / consistency"
    }

    fn plan(&self, s: &ScenarioSpec) -> Result<ScenarioPlan> {
        let count = s.count.unwrap_or(50);
        if count == 0 {
            bail!("scenario concurrent: count must be > 0");
        }
        let concurrency = s.concurrency.unwrap_or(4).max(2) as usize;
        let keys = s.keys.unwrap_or(1).max(1);

        // Round-robin keys, but the round-robin dispatch across producer threads
        // (done by the runner) is what actually interleaves same-key writes.
        let mut actions = Vec::with_capacity(count as usize);
        let mut per_key_i = vec![0u32; keys as usize];
        for seq in 0..count {
            let k = (seq % keys) as usize;
            let i = per_key_i[k];
            per_key_i[k] += 1;
            actions.push(action_for(s, seq, i, format!("k{k}")));
        }

        Ok(ScenarioPlan {
            actions,
            concurrency,
            per_message_delay_ms: delay_from_rate(s.rate_per_sec),
            summary: format!(
                "concurrent: {count} writes over {keys} key(s) across {concurrency} producers"
            ),
        })
    }
}
