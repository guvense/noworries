//! `burst` — throughput / no-loss under a flood.
//!
//! Produces `count` messages (default 100) as fast as possible (optionally rate
//! limited) over `keys` distinct keys (default: one key per message). Stresses
//! whether the pipeline keeps up and drops nothing. Verify with an exact count:
//! `db.expect_row_count: <count>` (or `mysql`/`elastic` equivalents).

use anyhow::{bail, Result};

use super::{action_for, delay_from_rate, EdgeCase, ScenarioPlan};
use crate::spec::ScenarioSpec;

pub struct Burst;

impl EdgeCase for Burst {
    fn kind(&self) -> &'static str {
        "burst"
    }

    fn describe(&self) -> &'static str {
        "flood of N messages — verifies throughput and that nothing is dropped"
    }

    fn plan(&self, s: &ScenarioSpec) -> Result<ScenarioPlan> {
        let count = s.count.unwrap_or(100);
        if count == 0 {
            bail!("scenario burst: count must be > 0");
        }
        let concurrency = s.concurrency.unwrap_or(1).max(1) as usize;
        // Default: every message is its own key (a wide fan-out that exercises
        // the whole keyspace); if `keys` is set, spread messages round-robin.
        let keys = s.keys.unwrap_or(count).max(1);

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
                "burst: {count} messages over {keys} key(s), {concurrency} producer(s)"
            ),
        })
    }
}
