//! Contiguous-frontier bookkeeping for out-of-order chunk completion.
//!
//! The durable checkpoint contract (see the pipelined-transfers design doc)
//! is that persisted progress NEVER counts bytes past a gap. This tracker is
//! the single source of that guarantee: `complete()` records finished ranges
//! and only advances `position` across fully covered bytes.

use std::collections::BTreeMap;

#[derive(Debug)]
pub(crate) struct Frontier {
    position: u64,
    /// Completed ranges beyond `position`, keyed by start offset.
    pending: BTreeMap<u64, u64>, // start -> end (exclusive)
}

impl Frontier {
    pub(crate) fn new(start: u64) -> Self {
        Self { position: start, pending: BTreeMap::new() }
    }

    pub(crate) fn position(&self) -> u64 {
        self.position
    }

    /// How many completed-but-not-yet-contiguous ranges are held back.
    /// Test-only introspection: the scheduler itself only ever asks for
    /// [`Frontier::position`].
    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Record a completed chunk and return the new contiguous frontier.
    pub(crate) fn complete(&mut self, offset: u64, len: u64) -> u64 {
        let end = offset.saturating_add(len);
        if end > self.position {
            let start = offset.max(self.position);
            let entry = self.pending.entry(start).or_insert(end);
            if *entry < end {
                *entry = end;
            }
        }
        // Fold every range that now touches the frontier.
        while let Some((&start, &end)) = self.pending.first_key_value() {
            if start > self.position {
                break;
            }
            self.pending.pop_first();
            if end > self.position {
                self.position = end;
            }
        }
        self.position
    }
}

#[cfg(test)]
mod tests {
    use super::Frontier;

    #[test]
    fn in_order_completions_advance_immediately() {
        let mut frontier = Frontier::new(0);
        assert_eq!(frontier.complete(0, 100), 100);
        assert_eq!(frontier.complete(100, 100), 200);
        assert_eq!(frontier.position(), 200);
        assert_eq!(frontier.pending(), 0);
    }

    #[test]
    fn out_of_order_completions_wait_for_the_gap() {
        let mut frontier = Frontier::new(0);
        assert_eq!(frontier.complete(200, 100), 0, "gap at 0..200 blocks the frontier");
        assert_eq!(frontier.complete(100, 100), 0, "gap at 0..100 still blocks");
        assert_eq!(frontier.pending(), 2);
        assert_eq!(frontier.complete(0, 100), 300, "filling the gap folds all pending ranges");
        assert_eq!(frontier.pending(), 0);
    }

    #[test]
    fn resume_start_offset_is_the_floor() {
        let mut frontier = Frontier::new(4096);
        assert_eq!(frontier.position(), 4096);
        assert_eq!(frontier.complete(4096, 512), 4608);
    }

    #[test]
    fn duplicate_and_overlapping_completions_do_not_double_count() {
        let mut frontier = Frontier::new(0);
        frontier.complete(0, 100);
        assert_eq!(frontier.complete(0, 100), 100, "exact duplicate is a no-op");
        assert_eq!(frontier.complete(50, 100), 150, "overlap only extends the uncovered part");
    }

    #[test]
    fn zero_length_completion_is_a_no_op() {
        let mut frontier = Frontier::new(10);
        assert_eq!(frontier.complete(10, 0), 10);
        assert_eq!(frontier.pending(), 0);
    }
}
