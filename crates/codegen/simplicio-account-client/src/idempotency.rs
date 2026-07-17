//! Pure webhook idempotency logic for `POST /webhooks/subscription`.
//!
//! The contract (`docs/contracts/site-simpleti-api.md`) requires at-least-once
//! delivery to be handled idempotently: the same `event_id` may be delivered
//! more than once, and it must be processed at most once. This module
//! contains no I/O and no network code — it only decides, given a
//! caller-supplied "have we seen this id" store, whether a given event
//! should be processed or skipped as a duplicate. A real integration plugs
//! in a persistent [`SeenEventStore`] (database, file, etc.); this crate
//! ships an in-memory one suitable for tests and for a single-process
//! client-side cache.

use std::collections::HashSet;

/// Tracks which webhook `event_id`s have already been processed.
///
/// Implementations must make `mark_seen` and `has_seen` consistent with each
/// other for a single logical store (i.e. after `mark_seen(id)`, a
/// subsequent `has_seen(id)` on the same store returns `true`).
pub trait SeenEventStore {
    /// Returns `true` if `event_id` has already been marked as processed.
    fn has_seen(&self, event_id: &str) -> bool;

    /// Records `event_id` as processed. Calling this more than once for the
    /// same id must be safe (idempotent) and must not change the outcome of
    /// `has_seen`.
    fn mark_seen(&mut self, event_id: String);
}

/// A simple in-memory [`SeenEventStore`]. Not persisted across process
/// restarts; a real deployment should back this with durable storage so a
/// restart does not reprocess a redelivered event.
#[derive(Debug, Default)]
pub struct InMemorySeenEventStore {
    seen: HashSet<String>,
}

impl InMemorySeenEventStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct event ids recorded so far.
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

impl SeenEventStore for InMemorySeenEventStore {
    fn has_seen(&self, event_id: &str) -> bool {
        self.seen.contains(event_id)
    }

    fn mark_seen(&mut self, event_id: String) {
        self.seen.insert(event_id);
    }
}

/// Outcome of [`decide`]: whether the caller should actually apply the
/// event's side effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// First time this `event_id` has been seen; the caller should process
    /// it and the store has already been updated to remember it.
    Process,
    /// This `event_id` was already processed; the caller must skip applying
    /// side effects again, but per the contract should still acknowledge
    /// (HTTP 200) so the sender stops retrying.
    SkipDuplicate,
}

/// Decides whether a webhook event with the given `event_id` should be
/// processed, given a [`SeenEventStore`]. On [`Decision::Process`], the
/// store has already been updated (`mark_seen`) so a second call with the
/// same id returns [`Decision::SkipDuplicate`].
///
/// This is the entire idempotency contract as pure logic: no HTTP, no
/// signature verification (that belongs to the transport layer that isn't
/// implemented yet), just the at-most-once decision.
pub fn decide(store: &mut dyn SeenEventStore, event_id: &str) -> Decision {
    if store.has_seen(event_id) {
        return Decision::SkipDuplicate;
    }
    store.mark_seen(event_id.to_string());
    Decision::Process
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_delivery_of_an_event_id_is_processed() {
        let mut store = InMemorySeenEventStore::new();
        assert_eq!(decide(&mut store, "evt_1"), Decision::Process);
    }

    #[test]
    fn redelivery_of_the_same_event_id_is_skipped() {
        let mut store = InMemorySeenEventStore::new();
        assert_eq!(decide(&mut store, "evt_1"), Decision::Process);
        assert_eq!(decide(&mut store, "evt_1"), Decision::SkipDuplicate);
        // A third redelivery must still be a duplicate, not processed again.
        assert_eq!(decide(&mut store, "evt_1"), Decision::SkipDuplicate);
    }

    #[test]
    fn distinct_event_ids_are_each_processed_once() {
        let mut store = InMemorySeenEventStore::new();
        assert_eq!(decide(&mut store, "evt_1"), Decision::Process);
        assert_eq!(decide(&mut store, "evt_2"), Decision::Process);
        assert_eq!(decide(&mut store, "evt_1"), Decision::SkipDuplicate);
        assert_eq!(decide(&mut store, "evt_2"), Decision::SkipDuplicate);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn out_of_order_redelivery_is_still_detected_as_duplicate() {
        // Simulates a sender retrying an older event after a newer one has
        // already been delivered and processed -- ordering is not assumed.
        let mut store = InMemorySeenEventStore::new();
        assert_eq!(decide(&mut store, "evt_2"), Decision::Process);
        assert_eq!(decide(&mut store, "evt_1"), Decision::Process);
        assert_eq!(decide(&mut store, "evt_2"), Decision::SkipDuplicate);
    }

    #[test]
    fn empty_store_reports_empty() {
        let store = InMemorySeenEventStore::new();
        assert!(store.is_empty());
        assert!(!store.has_seen("anything"));
    }

    /// A store double that lets a test assert exactly how many times
    /// mark_seen is invoked, independent of decide()'s own bookkeeping.
    struct CountingStore {
        inner: InMemorySeenEventStore,
        mark_seen_calls: usize,
    }

    impl SeenEventStore for CountingStore {
        fn has_seen(&self, event_id: &str) -> bool {
            self.inner.has_seen(event_id)
        }

        fn mark_seen(&mut self, event_id: String) {
            self.mark_seen_calls += 1;
            self.inner.mark_seen(event_id);
        }
    }

    #[test]
    fn duplicate_decisions_never_call_mark_seen_again() {
        let mut store = CountingStore {
            inner: InMemorySeenEventStore::new(),
            mark_seen_calls: 0,
        };
        decide(&mut store, "evt_1");
        decide(&mut store, "evt_1");
        decide(&mut store, "evt_1");
        assert_eq!(
            store.mark_seen_calls, 1,
            "a duplicate must not re-invoke mark_seen, only the first delivery should"
        );
    }
}
