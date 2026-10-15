//! SmartSort: a page of calls, newest first, with each call moved a few slots
//! towards a nearby call of the same talkgroup, so continuous play follows a
//! conversation instead of hopping between talkgroups.
//!
//! It is not a grouping. Walking from the oldest call up, for each of the
//! reaches 3, 2 and 1 it looks at the two calls that far and one further
//! towards the newest: when the further one is of the walking call's
//! talkgroup, the nearer one is not, and the further one began within
//! `GAP_SECONDS` of the nearer one's end, the two trade places. Swaps chain, so
//! a call can travel several slots, but only towards calls close to it in
//! time: the page still reads in time order, in conversation-sized runs.
//!
//! It works on the page's own calls only: which calls are on a page, the total
//! and the paging do not change with it.

/// The longest pause between two calls that SmartSort brings together.
pub const GAP_SECONDS: f64 = 30.0;

const REACHES: [usize; 3] = [3, 2, 1];

/// What SmartSort needs of a call.
pub trait Sortable {
    /// The talkgroup; `None` never matches another call.
    fn talkgroup(&self) -> Option<i64>;
    /// The start, in seconds of any one clock.
    fn start_seconds(&self) -> f64;
    fn length_seconds(&self) -> f64;
}

/// Reorders `page` (newest first) in place.
pub fn smart_sort<T: Sortable>(page: &mut [T]) {
    let same = |a: Option<i64>, b: Option<i64>| a.is_some() && a == b;
    for i in (0..page.len()).rev() {
        // Only calls nearer the newest than `i` move, so page[i] stays put.
        // The newest call of the page never moves.
        let walking = page[i].talkgroup();
        for k in REACHES {
            if i < k + 2 {
                continue;
            }
            let (near, far) = (i - k, i - k - 1);
            if !same(walking, page[near].talkgroup())
                && same(walking, page[far].talkgroup())
                && page[near].start_seconds() + GAP_SECONDS + page[near].length_seconds()
                    > page[far].start_seconds()
            {
                page.swap(near, far);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Sortable, smart_sort};

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Call {
        id: u32,
        talkgroup: Option<i64>,
        start: f64,
        length: f64,
    }

    impl Sortable for Call {
        fn talkgroup(&self) -> Option<i64> {
            self.talkgroup
        }
        fn start_seconds(&self) -> f64 {
            self.start
        }
        fn length_seconds(&self) -> f64 {
            self.length
        }
    }

    /// Calls given oldest first as (talkgroup, start); returned newest first.
    fn page(calls: &[(Option<i64>, f64)]) -> Vec<Call> {
        let mut page: Vec<Call> = calls
            .iter()
            .enumerate()
            .map(|(n, (talkgroup, start))| Call {
                id: n as u32 + 1,
                talkgroup: *talkgroup,
                start: *start,
                length: 2.0,
            })
            .collect();
        page.reverse();
        page
    }

    fn ids(page: &[Call]) -> Vec<u32> {
        page.iter().map(|c| c.id).collect()
    }

    #[test]
    fn a_reply_moves_next_to_its_conversation() {
        // 1 (A), 2 (B), 3 (A) a few seconds apart, then 4 (C): 3 moves next
        // to 1. The newest call, 4, stays first.
        let mut p = page(&[
            (Some(1), 0.0),
            (Some(2), 3.0),
            (Some(1), 6.0),
            (Some(3), 9.0),
        ]);
        assert_eq!(ids(&p), vec![4, 3, 2, 1]);
        smart_sort(&mut p);
        assert_eq!(ids(&p), vec![4, 2, 3, 1]);
    }

    #[test]
    fn the_newest_call_never_moves() {
        let mut p = page(&[(Some(1), 0.0), (Some(2), 3.0), (Some(1), 6.0)]);
        smart_sort(&mut p);
        assert_eq!(ids(&p), vec![3, 2, 1]);
    }

    #[test]
    fn calls_far_apart_in_time_stay_in_time_order() {
        let mut p = page(&[
            (Some(1), 0.0),
            (Some(2), 100.0),
            (Some(1), 200.0),
            (Some(3), 300.0),
        ]);
        smart_sort(&mut p);
        assert_eq!(ids(&p), vec![4, 3, 2, 1]);
    }

    #[test]
    fn it_is_not_a_grouping() {
        // Two conversations an hour apart on the same two talkgroups stay two
        // runs each, not one block per talkgroup.
        let mut p = page(&[
            (Some(1), 0.0),
            (Some(2), 3.0),
            (Some(1), 6.0),
            (Some(1), 3600.0),
            (Some(2), 3603.0),
            (Some(1), 3606.0),
            (Some(3), 3609.0),
        ]);
        smart_sort(&mut p);
        // Each conversation's talkgroup 1 calls come together (6 beside 4, 3
        // beside 1), and talkgroup 1 still shows as two runs, one per
        // conversation, not as one block.
        assert_eq!(ids(&p), vec![7, 6, 4, 5, 2, 3, 1]);
    }

    #[test]
    fn calls_without_a_talkgroup_never_move_together() {
        let mut p = page(&[(None, 0.0), (Some(2), 3.0), (None, 6.0), (Some(3), 9.0)]);
        smart_sort(&mut p);
        assert_eq!(ids(&p), vec![4, 3, 2, 1]);
    }

    #[test]
    fn the_page_keeps_its_calls() {
        let mut p = page(&[
            (Some(1), 0.0),
            (Some(2), 1.0),
            (Some(3), 2.0),
            (Some(1), 3.0),
            (Some(2), 4.0),
            (Some(1), 5.0),
        ]);
        smart_sort(&mut p);
        let mut sorted = ids(&p);
        sorted.sort();
        assert_eq!(sorted, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(page(&[]).len(), 0);
        let mut empty: Vec<Call> = Vec::new();
        smart_sort(&mut empty);
    }
}
