//! `out_of_order` — late / reordered events.
//!
//! For each key, produces a logical sequence but emits it in **reverse arrival
//! order**: the message with the highest logical index `${i}` is sent first, the
//! oldest last. The true order still travels in the payload (via `${i}` — use it
//! as a version/event-time field), so a correct pipeline reorders and ends in the
//! same final state as if events had arrived in order. Verify the final value
//! equals the highest `${i}` per key; a naive last-arrival-wins bug ends on the
//! *oldest* event instead.

use anyhow::{bail, Result};

use super::{action_for, delay_from_rate, EdgeCase, ScenarioPlan};
use crate::spec::ScenarioSpec;

pub struct OutOfOrder;

impl EdgeCase for OutOfOrder {
    fn kind(&self) -> &'static str {
        "out_of_order"
    }

    fn describe(&self) -> &'static str {
        "events emitted in reverse order — verifies windowing / reordering correctness"
    }

    fn plan(&self, s: &ScenarioSpec) -> Result<ScenarioPlan> {
        let count = s.count.unwrap_or(50);
        if count == 0 {
            bail!("scenario out_of_order: count must be > 0");
        }
        // Sequential arrival matters here, so a single producer by default keeps
        // the reversed order intact end to end.
        let concurrency = s.concurrency.unwrap_or(1).max(1) as usize;
        let keys = s.keys.unwrap_or(1).max(1);

        // Distribute `count` messages across `keys` as evenly as possible.
        let base = count / keys;
        let rem = count % keys;

        let mut actions = Vec::with_capacity(count as usize);
        let mut seq = 0u32;
        for k in 0..keys {
            let per_key = base + if k < rem { 1 } else { 0 };
            // Emit highest logical index first (reverse arrival).
            for logical in (0..per_key).rev() {
                actions.push(action_for(s, seq, logical, format!("k{k}")));
                seq += 1;
            }
        }

        Ok(ScenarioPlan {
            actions,
            concurrency,
            per_message_delay_ms: delay_from_rate(s.rate_per_sec),
            summary: format!(
                "out_of_order: {count} messages in reverse per-key order over {keys} key(s)"
            ),
        })
    }
}
