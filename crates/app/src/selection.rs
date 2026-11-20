//! The selection of a track list (or the queue): which rows are chosen, as file managers and
//! players do it. A click chooses one row, Ctrl+click adds or takes away one, Shift+click the
//! rows from the anchor (the last row clicked without Shift) to it, Ctrl+A every row, Esc none;
//! the arrows move it, Shift and the arrows stretch it from the anchor.
//!
//! Rows are known by a key that stays with the row while the list's contents stay the same
//! (`rows::TrackModel` makes new keys when its tracks change; the queue's are its items' uids),
//! never by an index alone: the anchor and what a menu was opened on are keys, and an action
//! checks them again before it acts. Each row's state is one flag beside its key, so a click
//! flips flags and says which rows changed (to be drawn again): nothing is rebuilt.
//!
//! A press on a row already chosen waits for the release before it acts (a click there chooses
//! that row alone; with Ctrl, takes it away), so that a drag from it carries every chosen row.

use std::collections::HashSet;

/// A press on a chosen row, acted on at its release (a drag in between drops it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Deferred {
    /// Choose this row alone.
    Only,
    /// Take this row away.
    Remove,
}

#[derive(Debug, Default)]
pub struct Selection {
    keys: Vec<u64>,
    on: Vec<bool>,
    count: usize,
    anchor: Option<u64>,
    pending: Option<(u64, Deferred)>,
}

impl Selection {
    /// New contents: these rows, none chosen.
    pub fn reset(&mut self, keys: Vec<u64>) {
        self.on.clear();
        self.on.resize(keys.len(), false);
        self.keys = keys;
        self.count = 0;
        self.anchor = None;
        self.pending = None;
    }

    /// The same items, some moved, added or gone (the queue): the rows still there keep their
    /// state, the anchor too if it is still there.
    pub fn rekey(&mut self, keys: Vec<u64>) {
        if keys == self.keys {
            return;
        }
        let chosen: HashSet<u64> = self.selected_keys().into_iter().collect();
        self.on.clear();
        self.on.extend(keys.iter().map(|k| chosen.contains(k)));
        self.count = self.on.iter().filter(|o| **o).count();
        if self.anchor.is_some_and(|a| !keys.contains(&a)) {
            self.anchor = None;
        }
        self.pending = None;
        self.keys = keys;
    }

    #[cfg(test)]
    pub fn keys(&self) -> &[u64] {
        &self.keys
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn is_selected(&self, row: usize) -> bool {
        self.on.get(row).copied().unwrap_or(false)
    }

    /// The chosen rows, in list order.
    pub fn rows(&self) -> Vec<usize> {
        self.on.iter().enumerate().filter(|(_, o)| **o).map(|(i, _)| i).collect()
    }

    /// The chosen rows' keys, in list order.
    pub fn selected_keys(&self) -> Vec<u64> {
        self.rows().into_iter().map(|i| self.keys[i]).collect()
    }

    fn set(&mut self, row: usize, on: bool, changed: &mut Vec<usize>) {
        if let Some(o) = self.on.get_mut(row)
            && *o != on
        {
            *o = on;
            if on {
                self.count += 1;
            } else {
                self.count -= 1;
            }
            changed.push(row);
        }
    }

    fn anchor_row(&self) -> Option<usize> {
        let a = self.anchor?;
        self.keys.iter().position(|k| *k == a)
    }

    fn set_anchor(&mut self, row: usize) {
        self.anchor = self.keys.get(row).copied();
    }

    /// Only these rows chosen.
    fn only_rows(&mut self, keep: impl Fn(usize) -> bool, changed: &mut Vec<usize>) {
        for i in 0..self.on.len() {
            self.set(i, keep(i), changed);
        }
    }

    /// `row` alone, the anchor on it.
    pub fn only(&mut self, row: usize, changed: &mut Vec<usize>) {
        if row >= self.keys.len() {
            return;
        }
        self.only_rows(|i| i == row, changed);
        self.set_anchor(row);
    }

    /// From the anchor (`row` when there is none) to `row`; with `add`, beside what is chosen.
    fn range(&mut self, row: usize, add: bool, changed: &mut Vec<usize>) {
        let a = self.anchor_row().unwrap_or(row);
        let (lo, hi) = (a.min(row), a.max(row));
        if add {
            for i in lo..=hi {
                self.set(i, true, changed);
            }
        } else {
            self.only_rows(|i| (lo..=hi).contains(&i), changed);
        }
        if self.anchor.is_none() {
            self.set_anchor(row);
        }
    }

    /// The left button down on `row`.
    pub fn press(&mut self, row: usize, ctrl: bool, shift: bool, changed: &mut Vec<usize>) {
        self.pending = None;
        if row >= self.keys.len() {
            return;
        }
        let key = self.keys[row];
        if shift {
            self.range(row, ctrl, changed);
        } else if self.is_selected(row) {
            self.pending = Some((key, if ctrl { Deferred::Remove } else { Deferred::Only }));
            if ctrl {
                self.anchor = Some(key);
            }
        } else if ctrl {
            self.set(row, true, changed);
            self.anchor = Some(key);
        } else {
            self.only(row, changed);
        }
    }

    /// The press on `row` ended as a click (no drag).
    pub fn release(&mut self, row: usize, changed: &mut Vec<usize>) {
        let Some((key, what)) = self.pending.take() else { return };
        if self.keys.get(row) != Some(&key) {
            return;
        }
        match what {
            Deferred::Only => self.only(row, changed),
            Deferred::Remove => self.set(row, false, changed),
        }
    }

    /// A drag happened: a press waiting for its release does nothing. (In the app the release
    /// of a drag never reaches the row as a click, and the next press starts afresh.)
    #[cfg(test)]
    pub fn cancel_pending(&mut self) {
        self.pending = None;
    }

    /// A right click (or the Menu key) on `row`: a chosen row keeps the selection, another is
    /// chosen alone. True when `row` is part of what is chosen now.
    pub fn context(&mut self, row: usize, changed: &mut Vec<usize>) -> bool {
        self.pending = None;
        if row >= self.keys.len() {
            return false;
        }
        if !self.is_selected(row) {
            self.only(row, changed);
        }
        true
    }

    /// The keyboard cursor moved from `from` to `to`: `to` alone, or with `extend` (Shift) the
    /// rows from the anchor (`from` when there is none) to it.
    pub fn cursor(&mut self, from: usize, to: usize, extend: bool, changed: &mut Vec<usize>) {
        self.pending = None;
        if to >= self.keys.len() {
            return;
        }
        if extend {
            if self.anchor_row().is_none() {
                self.set_anchor(from.min(self.keys.len() - 1));
            }
            self.range(to, false, changed);
        } else {
            self.only(to, changed);
        }
    }

    /// Every row; the anchor stays where it was (on the first row if there was none).
    pub fn all(&mut self, changed: &mut Vec<usize>) {
        self.pending = None;
        self.only_rows(|_| true, changed);
        if self.anchor_row().is_none() && !self.keys.is_empty() {
            self.set_anchor(0);
        }
    }

    /// None chosen. True when some were.
    pub fn clear(&mut self, changed: &mut Vec<usize>) -> bool {
        self.pending = None;
        let had = self.count > 0;
        if had {
            self.only_rows(|_| false, changed);
        }
        self.anchor = None;
        had
    }

    /// Exactly these rows (a block moved: where it is now), the anchor on the first.
    pub fn select_rows(&mut self, rows: std::ops::Range<usize>, changed: &mut Vec<usize>) {
        self.pending = None;
        self.only_rows(|i| rows.contains(&i), changed);
        if rows.start < self.keys.len() {
            self.set_anchor(rows.start);
        }
    }
}

/// Moving the rows `rows` (in list order, no repeats) of a list of `n` as one block, dropped on
/// the line before row `at` (0..=n, in the list before the move, as the drop line shows it):
/// the new order, as old indices, and where the block starts in it. The block keeps its order;
/// the rows around it keep theirs. Rows out of range are left where they are.
pub fn move_block(n: usize, rows: &[usize], at: usize) -> (Vec<usize>, usize) {
    let at = at.min(n);
    let mut moved = vec![false; n];
    let block: Vec<usize> = rows.iter().copied().filter(|&r| r < n && !std::mem::replace(&mut moved[r], true)).collect();
    let rest: Vec<usize> = (0..n).filter(|&i| !moved[i]).collect();
    // The line before `at` among the rows that stay.
    let start = rest.iter().take_while(|&&i| i < at).count();
    let mut order = Vec::with_capacity(n);
    order.extend_from_slice(&rest[..start]);
    order.extend_from_slice(&block);
    order.extend_from_slice(&rest[start..]);
    (order, start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(n: u64) -> Selection {
        let mut s = Selection::default();
        s.reset((100..100 + n).collect());
        s
    }

    fn click(s: &mut Selection, row: usize, ctrl: bool, shift: bool) -> Vec<usize> {
        let mut changed = Vec::new();
        s.press(row, ctrl, shift, &mut changed);
        s.release(row, &mut changed);
        changed
    }

    #[test]
    fn a_click_chooses_one_row() {
        let mut s = list(5);
        assert_eq!(click(&mut s, 2, false, false), [2]);
        assert_eq!(s.rows(), [2]);
        let mut changed = click(&mut s, 4, false, false);
        changed.sort_unstable();
        assert_eq!(changed, [2, 4], "only the rows that changed are drawn again");
        assert_eq!(s.rows(), [4]);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn ctrl_click_toggles_a_row() {
        let mut s = list(6);
        click(&mut s, 1, false, false);
        click(&mut s, 3, true, false);
        click(&mut s, 5, true, false);
        assert_eq!(s.rows(), [1, 3, 5]);
        click(&mut s, 3, true, false);
        assert_eq!(s.rows(), [1, 5]);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn shift_click_takes_the_range_from_a_moving_anchor() {
        let mut s = list(10);
        click(&mut s, 2, false, false);
        click(&mut s, 5, false, true);
        assert_eq!(s.rows(), [2, 3, 4, 5]);
        // Shift again: the range from the same anchor, the other way, replacing the last.
        click(&mut s, 0, false, true);
        assert_eq!(s.rows(), [0, 1, 2]);
        // Ctrl+click moves the anchor; Shift+click then goes from there.
        click(&mut s, 7, true, false);
        assert_eq!(s.rows(), [0, 1, 2, 7]);
        click(&mut s, 9, false, true);
        assert_eq!(s.rows(), [7, 8, 9]);
        // Ctrl+Shift adds the range to what is chosen.
        click(&mut s, 4, false, false);
        click(&mut s, 1, true, false);
        click(&mut s, 3, true, true);
        assert_eq!(s.rows(), [1, 2, 3, 4]);
        // With no anchor yet, Shift+click is a plain click.
        let mut t = list(4);
        click(&mut t, 2, false, true);
        assert_eq!(t.rows(), [2]);
    }

    #[test]
    fn a_press_on_a_chosen_row_waits_for_the_click() {
        let mut s = list(5);
        click(&mut s, 1, false, false);
        click(&mut s, 3, false, true);
        let mut changed = Vec::new();
        // Pressed: the selection stays (a drag from here carries all three).
        s.press(2, false, false, &mut changed);
        assert_eq!(s.rows(), [1, 2, 3]);
        assert!(changed.is_empty());
        // A drag happened: the release does nothing.
        s.cancel_pending();
        s.release(2, &mut changed);
        assert_eq!(s.rows(), [1, 2, 3]);
        // A click: that row alone.
        click(&mut s, 2, false, false);
        assert_eq!(s.rows(), [2]);
        // A release on another row than the press does nothing either.
        click(&mut s, 4, true, false);
        s.press(2, false, false, &mut changed);
        s.release(4, &mut changed);
        assert_eq!(s.rows(), [2, 4]);
    }

    #[test]
    fn select_all_and_clear() {
        let mut s = list(4);
        let mut changed = Vec::new();
        s.all(&mut changed);
        assert_eq!(s.rows(), [0, 1, 2, 3]);
        assert_eq!(changed.len(), 4);
        changed.clear();
        s.all(&mut changed);
        assert!(changed.is_empty(), "nothing to draw again");
        assert!(s.clear(&mut changed));
        assert!(s.is_empty());
        assert!(!s.clear(&mut changed), "Esc with nothing chosen goes on (Back)");
        // Shift+click after Ctrl+A: from the first row (the anchor Ctrl+A leaves).
        s.all(&mut changed);
        click(&mut s, 2, false, true);
        assert_eq!(s.rows(), [0, 1, 2]);
    }

    #[test]
    fn the_keyboard_moves_and_stretches_it() {
        let mut s = list(8);
        let mut c = Vec::new();
        s.cursor(0, 3, false, &mut c);
        assert_eq!(s.rows(), [3]);
        s.cursor(3, 4, true, &mut c);
        s.cursor(4, 5, true, &mut c);
        assert_eq!(s.rows(), [3, 4, 5]);
        s.cursor(5, 2, true, &mut c);
        assert_eq!(s.rows(), [2, 3]);
        s.cursor(2, 6, false, &mut c);
        assert_eq!(s.rows(), [6]);
        // Shift with nothing chosen: from where the cursor was.
        s.clear(&mut c);
        s.cursor(1, 2, true, &mut c);
        assert_eq!(s.rows(), [1, 2]);
    }

    #[test]
    fn a_right_click_keeps_a_selection_it_is_in() {
        let mut s = list(6);
        click(&mut s, 1, false, false);
        click(&mut s, 4, false, true);
        let mut c = Vec::new();
        assert!(s.context(2, &mut c));
        assert_eq!(s.rows(), [1, 2, 3, 4]);
        assert!(s.context(5, &mut c));
        assert_eq!(s.rows(), [5], "outside it: that row alone");
        assert!(!s.context(9, &mut c));
    }

    #[test]
    fn new_contents_clear_it_and_the_queue_keeps_what_stays() {
        let mut s = list(5);
        click(&mut s, 1, false, false);
        click(&mut s, 3, false, true);
        s.reset(vec![7, 8, 9]);
        assert!(s.is_empty());
        assert_eq!(s.keys(), [7, 8, 9]);
        // Rekeyed (the queue): moved rows keep their state, a removed one goes.
        let mut q = Selection::default();
        q.reset(vec![10, 11, 12, 13]);
        click(&mut q, 1, false, false);
        click(&mut q, 2, true, false);
        q.rekey(vec![12, 10, 13, 14]);
        assert_eq!(q.selected_keys(), [12]);
        assert_eq!(q.rows(), [0]);
        assert_eq!(q.len(), 1);
        // The anchor (12, the last Ctrl+click) moved with its row.
        click(&mut q, 3, false, true);
        assert_eq!(q.rows(), [0, 1, 2, 3]);
    }

    #[test]
    fn a_block_moves_keeping_its_order() {
        // a b c d e f; move b and d to the line before f (5): a c e b d f.
        let (order, start) = move_block(6, &[1, 3], 5);
        assert_eq!(order, [0, 2, 4, 1, 3, 5]);
        assert_eq!(start, 3);
        // Up, to the top.
        let (order, start) = move_block(6, &[2, 4], 0);
        assert_eq!(order, [2, 4, 0, 1, 3, 5]);
        assert_eq!(start, 0);
        // To the end.
        assert_eq!(move_block(4, &[0, 1], 4), (vec![2, 3, 0, 1], 2));
        // Dropped on a line inside the block (or at its edges): it closes up there.
        assert_eq!(move_block(5, &[1, 3], 2), (vec![0, 1, 3, 2, 4], 1));
        assert_eq!(move_block(5, &[1, 2], 1), (vec![0, 1, 2, 3, 4], 1));
        assert_eq!(move_block(5, &[1, 2], 3), (vec![0, 1, 2, 3, 4], 1));
        // One row: as `dnd::move_target` has it.
        for (from, at) in [(0, 3), (3, 0), (2, 2), (2, 3)] {
            let (order, start) = move_block(5, &[from], at);
            assert_eq!(start, crate::dnd::move_target(from, at));
            assert_eq!(order[start], from);
        }
        // Repeats and rows out of range are ignored; `at` past the end is the end.
        assert_eq!(move_block(3, &[0, 0, 7], 9), (vec![1, 2, 0], 2));
    }
}
