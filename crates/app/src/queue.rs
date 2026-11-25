//! The play queue: an ordered list with a current position. Pure logic, no UI and no
//! engine, so it is unit-tested on its own.

use library::Track;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn cycle(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// Unique per queue entry, stable while it moves around.
    pub uid: u64,
    pub track: Track,
}

/// Shuffle is a mode. Turning it on remembers the order (by uid) and shuffles what is
/// still to come; the playing item keeps playing. Turning it off puts the remembered
/// order back and playback continues from the playing item's place in it.
///
/// While shuffle is on:
/// - Items added go back, when it is turned off, right after the item they followed in
///   the shuffled order. "Play next" therefore still plays next after turning it off.
/// - Items moved by hand return to their original place when it is turned off: the
///   original order wins. Removed items stay removed.
/// - A new queue (playing an album) is remembered as its own original order and
///   shuffled, since the mode stays on.
#[derive(Debug, Default)]
pub struct Queue {
    items: Vec<Item>,
    current: Option<usize>,
    next_uid: u64,
    pub repeat: Repeat,
    /// The order before shuffle, while shuffle is on.
    original: Option<Vec<u64>>,
    rng: u64,
    /// Bumped by every change to the items, their order or the shuffle state (not by
    /// the current item moving), so the saved queue is rewritten only when it changed.
    revision: u64,
}

impl Queue {
    fn make(&mut self, tracks: Vec<Track>) -> Vec<Item> {
        tracks
            .into_iter()
            .map(|track| {
                self.next_uid += 1;
                Item { uid: self.next_uid, track }
            })
            .collect()
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Replace each item's track data with `fresh(track id)` where that differs (a
    /// rescan moved, renamed or retagged it; the ids stay). Items it returns `None` for
    /// are left as they are. Returns (any changed, the current one changed).
    pub fn update_tracks(&mut self, fresh: impl Fn(i64) -> Option<Track>) -> (bool, bool) {
        let (mut any, mut current) = (false, false);
        for (i, item) in self.items.iter_mut().enumerate() {
            if let Some(t) = fresh(item.track.id)
                && t != item.track
            {
                item.track = t;
                any = true;
                current |= self.current == Some(i);
            }
        }
        (any, current)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn touch(&mut self) {
        self.revision += 1;
    }

    /// What `restore` needs: each item's track id and, while shuffled, its place in the
    /// order from before shuffle (`None` for items added while shuffled).
    pub fn snapshot(&self) -> Vec<(i64, Option<i64>)> {
        let rank: std::collections::HashMap<u64, i64> = self
            .original
            .iter()
            .flatten()
            .enumerate()
            .map(|(i, u)| (*u, i as i64))
            .collect();
        self.items.iter().map(|i| (i.track.id, rank.get(&i.uid).copied())).collect()
    }

    /// A queue as saved by `snapshot` (minus tracks that are gone). With `shuffled`, the
    /// items that have a place in the original order are remembered in that order, so
    /// turning shuffle off restores it; if none has one, the order as given is the
    /// original.
    pub fn restore(items: Vec<(Track, Option<i64>)>, current: Option<usize>, shuffled: bool, repeat: Repeat, seed: u64) -> Self {
        let mut q = Queue { repeat, rng: seed, ..Default::default() };
        let ranks: Vec<Option<i64>> = items.iter().map(|(_, r)| *r).collect();
        q.items = q.make(items.into_iter().map(|(t, _)| t).collect());
        q.current = current.filter(|&c| c < q.items.len());
        if shuffled {
            let mut known: Vec<(i64, u64)> =
                q.items.iter().zip(&ranks).filter_map(|(i, r)| r.map(|r| (r, i.uid))).collect();
            if known.is_empty() {
                q.original = Some(q.uids());
            } else {
                known.sort_by_key(|(r, _)| *r);
                q.original = Some(known.into_iter().map(|(_, u)| u).collect());
            }
        }
        q
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    pub fn current(&self) -> Option<&Item> {
        self.current.and_then(|i| self.items.get(i))
    }

    /// Replace the queue and make `start` current. Under shuffle, what follows `start` is
    /// shuffled and the given order is remembered.
    pub fn replace(&mut self, tracks: Vec<Track>, start: usize) -> Option<&Item> {
        self.touch();
        self.items = self.make(tracks);
        self.current = (start < self.items.len()).then_some(start);
        if self.original.is_some() {
            self.original = Some(self.uids());
            let seed = self.next_seed();
            self.shuffle_upcoming(seed);
        }
        self.current()
    }

    fn uids(&self) -> Vec<u64> {
        self.items.iter().map(|i| i.uid).collect()
    }

    fn current_uid(&self) -> Option<u64> {
        self.current().map(|i| i.uid)
    }

    fn restore_current(&mut self, uid: Option<u64>) {
        self.current = uid.and_then(|u| self.items.iter().position(|i| i.uid == u));
    }

    fn next_seed(&mut self) -> u64 {
        self.rng = self.rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.rng
    }

    pub fn shuffled(&self) -> bool {
        self.original.is_some()
    }

    /// Turn shuffle on or off (see the type's doc). `seed` varies the order.
    pub fn set_shuffle(&mut self, on: bool, seed: u64) {
        self.touch();
        match (on, self.original.is_some()) {
            (true, false) => {
                self.original = Some(self.uids());
                self.rng = seed;
                let s = self.next_seed();
                self.shuffle_upcoming(s);
            }
            (false, true) => self.restore_original(),
            _ => {}
        }
    }

    fn restore_original(&mut self) {
        let Some(original) = self.original.take() else { return };
        let uid = self.current_uid();
        let rank: std::collections::HashMap<u64, usize> = original.iter().enumerate().map(|(i, u)| (*u, i)).collect();
        // Known items in their original order.
        let mut out: Vec<Item> = self.items.iter().filter(|i| rank.contains_key(&i.uid)).cloned().collect();
        out.sort_by_key(|i| rank[&i.uid]);
        // Items added while shuffled: after the item they followed in the shuffled order.
        for (k, item) in self.items.iter().enumerate() {
            if rank.contains_key(&item.uid) {
                continue;
            }
            let at = match k.checked_sub(1).map(|p| self.items[p].uid) {
                Some(pred) => out.iter().position(|i| i.uid == pred).map_or(out.len(), |p| p + 1),
                None => 0,
            };
            out.insert(at, item.clone());
        }
        self.items = out;
        self.restore_current(uid);
    }

    /// Insert at `index` (clamped to the end). The current item stays current.
    pub fn insert_at(&mut self, index: usize, tracks: Vec<Track>) -> usize {
        self.touch();
        let uid = self.current_uid();
        let items = self.make(tracks);
        let at = index.min(self.items.len());
        self.items.splice(at..at, items);
        self.restore_current(uid);
        at
    }

    /// Move the item at `from` so it ends up at index `to` (in the list after the move).
    /// The current item stays current.
    pub fn move_item(&mut self, from: usize, to: usize) {
        if from >= self.items.len() {
            return;
        }
        self.touch();
        let uid = self.current_uid();
        let item = self.items.remove(from);
        let to = to.min(self.items.len());
        self.items.insert(to, item);
        self.restore_current(uid);
    }

    /// Move the items at `rows` as one block, in their order, to the line before `at` (in the
    /// list before the move: `selection::move_block`). Where the block starts now. The current
    /// item stays current.
    pub fn move_block(&mut self, rows: &[usize], at: usize) -> usize {
        let (order, start) = crate::selection::move_block(self.items.len(), rows, at);
        if order.iter().enumerate().all(|(i, &j)| i == j) {
            return start;
        }
        self.touch();
        let uid = self.current_uid();
        let mut old: Vec<Option<Item>> = std::mem::take(&mut self.items).into_iter().map(Some).collect();
        self.items = order.into_iter().filter_map(|j| old[j].take()).collect();
        self.restore_current(uid);
        start
    }

    /// Where the first of them is now (as `insert_next` and `insert_at` say).
    pub fn append(&mut self, tracks: Vec<Track>) -> usize {
        self.touch();
        let at = self.items.len();
        let items = self.make(tracks);
        self.items.extend(items);
        at
    }

    /// Insert right after the current item (at the end when there is none).
    pub fn insert_next(&mut self, tracks: Vec<Track>) -> usize {
        self.touch();
        let items = self.make(tracks);
        let at = self.current.map_or(self.items.len(), |c| c + 1);
        self.items.splice(at..at, items);
        at
    }

    #[cfg(test)]
    pub fn remove(&mut self, index: usize) {
        self.remove_many(&[index]);
    }

    /// Remove the items at `rows` (any order; repeats and rows past the end are ignored). If
    /// the current one goes, the first item after it that stays takes its place.
    pub fn remove_many(&mut self, rows: &[usize]) {
        let n = self.items.len();
        let mut gone = vec![false; n];
        for &r in rows {
            if r < n {
                gone[r] = true;
            }
        }
        if !gone.contains(&true) {
            return;
        }
        self.touch();
        self.current = self.current.and_then(|c| {
            let before = gone[..c].iter().filter(|g| **g).count();
            let left = n - gone.iter().filter(|g| **g).count();
            let at = c - before;
            (at < left).then_some(at)
        });
        let mut i = 0;
        self.items.retain(|_| {
            let keep = !gone[i];
            i += 1;
            keep
        });
    }

    pub fn clear(&mut self) {
        self.touch();
        self.items.clear();
        self.current = None;
        if self.original.is_some() {
            self.original = Some(Vec::new());
        }
    }

    pub fn jump(&mut self, index: usize) -> Option<&Item> {
        if index < self.items.len() {
            self.current = Some(index);
        }
        self.current()
    }

    pub fn set_current_uid(&mut self, uid: u64) {
        if let Some(i) = self.items.iter().position(|it| it.uid == uid) {
            self.current = Some(i);
        }
    }

    /// The item that follows the current one under the repeat mode, without moving.
    pub fn peek_next(&self) -> Option<&Item> {
        let c = self.current?;
        match self.repeat {
            Repeat::One => self.items.get(c),
            Repeat::All if c + 1 >= self.items.len() => self.items.first(),
            _ => self.items.get(c + 1),
        }
    }

    /// Move to the next item for a user skip. Repeat-one does not trap a skip.
    pub fn skip_next(&mut self) -> Option<&Item> {
        let c = self.current?;
        let n = if c + 1 < self.items.len() {
            c + 1
        } else if self.repeat != Repeat::Off {
            0
        } else {
            return None;
        };
        self.current = Some(n);
        self.current()
    }

    pub fn skip_previous(&mut self) -> Option<&Item> {
        let c = self.current?;
        let p = if c > 0 {
            c - 1
        } else if self.repeat != Repeat::Off {
            self.items.len().checked_sub(1)?
        } else {
            0
        };
        self.current = Some(p);
        self.current()
    }

    /// Shuffle everything after the current item.
    pub fn shuffle_upcoming(&mut self, seed: u64) {
        self.touch();
        let start = self.current.map_or(0, |c| c + 1);
        let tail = &mut self.items[start..];
        // Fisher-Yates: every order as likely as any other. The numbers are splitmix64's
        // (good for any seed, zero or a small count included).
        let mut state = seed;
        for i in (1..tail.len()).rev() {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            tail.swap(i, below(z ^ (z >> 31), i + 1));
        }
    }
}

/// `x` (64 random bits) as an index below `n`: its high bits scaled, not a remainder
/// (which takes the low bits and favours small indexes). The bias left is under n / 2^64.
pub fn below(x: u64, n: usize) -> usize {
    ((u128::from(x) * n as u128) >> 64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seeds that differ little (a clock's, counted in hundreds) still give every order of
    /// a short queue about as often, and `below` every index.
    #[test]
    fn shuffle_is_even_for_poor_seeds() {
        let mut orders = std::collections::HashMap::new();
        let mut starts = [0usize; 10];
        let runs = 24_000u64;
        for k in 0..runs {
            let mut q = Queue::default();
            q.replace((1..=5).map(t).collect(), 0);
            q.set_shuffle(true, k * 100);
            *orders.entry(q.items().iter().map(|i| i.track.id).collect::<Vec<_>>()).or_insert(0u64) += 1;
            starts[below(k.wrapping_mul(0x9E37_79B9_7F4A_7C15), 10)] += 1;
        }
        // 24 orders of the four after the current one, 1000 each on average.
        assert_eq!(orders.len(), 24);
        assert!(orders.values().all(|&n| (850..1150).contains(&n)), "{:?}", orders.values().collect::<Vec<_>>());
        assert!(starts.iter().all(|&n| (2300..2500).contains(&n)), "{starts:?}");
    }

    fn t(id: i64) -> Track {
        Track {
            id,
            album_id: 1,
            path: format!("{id}.opus"),
            title: format!("t{id}"),
            artist: "a".into(),
            album: "x".into(),
            album_artist: "a".into(),
            disc_no: None,
            track_no: None,
            duration: 1.0,
            art: None,
            ..Track::default()
        }
    }

    fn ids(q: &Queue) -> Vec<i64> {
        q.items().iter().map(|i| i.track.id).collect()
    }

    /// Tracks added to a queue that has items but no current one (its last current track
    /// removed) land after them, and say where, for playback to start at them.
    #[test]
    fn tracks_added_with_no_current_item_say_where_they_landed() {
        let mut q = Queue::default();
        q.replace(vec![t(1), t(2), t(3)], 2);
        q.remove(2);
        assert!(q.current().is_none());
        let at = q.append(vec![t(4)]);
        assert_eq!(q.jump(at).unwrap().track.id, 4);
        q.remove(at);
        let at = q.insert_next(vec![t(5), t(6)]);
        assert_eq!(q.jump(at).unwrap().track.id, 5);
        q.remove_many(&[at, at + 1]);
        let at = q.insert_at(1, vec![t(7)]);
        assert_eq!(q.jump(at).unwrap().track.id, 7);
        assert_eq!(ids(&q), [1, 7, 2]);
    }

    #[test]
    fn replace_and_advance() {
        let mut q = Queue::default();
        q.replace(vec![t(1), t(2), t(3)], 1);
        assert_eq!(q.current().unwrap().track.id, 2);
        assert_eq!(q.peek_next().unwrap().track.id, 3);
        assert_eq!(q.skip_next().unwrap().track.id, 3);
        assert!(q.peek_next().is_none());
        assert!(q.skip_next().is_none());
        q.repeat = Repeat::All;
        assert_eq!(q.peek_next().unwrap().track.id, 1);
        q.repeat = Repeat::One;
        assert_eq!(q.peek_next().unwrap().track.id, 3);
        assert_eq!(q.skip_next().unwrap().track.id, 1, "skip escapes repeat-one");
    }

    #[test]
    fn insert_next_goes_after_current() {
        let mut q = Queue::default();
        q.replace(vec![t(1), t(2), t(3)], 0);
        q.insert_next(vec![t(9)]);
        assert_eq!(ids(&q), vec![1, 9, 2, 3]);
        let mut e = Queue::default();
        e.insert_next(vec![t(5)]);
        assert_eq!(ids(&e), vec![5]);
    }

    #[test]
    fn remove_keeps_current_pointing_at_the_same_item() {
        let mut q = Queue::default();
        q.replace(vec![t(1), t(2), t(3), t(4)], 2);
        q.remove(0);
        assert_eq!(q.current().unwrap().track.id, 3);
        q.remove(3);
        q.remove(1); // the current one: the next item takes its place
        assert_eq!(q.current().unwrap().track.id, 4);
    }

    #[test]
    fn shuffle_toggle_restores_order_and_keeps_the_current_track() {
        let mut q = Queue::default();
        q.replace((1..=10).map(t).collect(), 3);
        q.set_shuffle(true, 7);
        assert!(q.shuffled());
        assert_eq!(&ids(&q)[..4], &[1, 2, 3, 4], "played part and current stay");
        assert_ne!(&ids(&q)[4..], &[5, 6, 7, 8, 9, 10]);
        assert_eq!(q.current().unwrap().track.id, 4);
        // Play on two tracks in shuffled order, then turn shuffle off.
        q.skip_next();
        q.skip_next();
        let playing = q.current().unwrap().track.id;
        q.set_shuffle(false, 0);
        assert!(!q.shuffled());
        assert_eq!(ids(&q), (1..=10).collect::<Vec<_>>());
        assert_eq!(q.current().unwrap().track.id, playing, "the playing track keeps playing");
        assert_eq!(q.current_index(), Some(playing as usize - 1), "and continues from its original place");
    }

    #[test]
    fn items_added_while_shuffled_follow_their_predecessor() {
        let mut q = Queue::default();
        q.replace((1..=6).map(t).collect(), 0);
        q.set_shuffle(true, 99);
        q.insert_next(vec![t(50)]); // "play next" while shuffled
        q.append(vec![t(60)]);
        let shuffled = ids(&q);
        let before_60 = shuffled[shuffled.len() - 2];
        q.set_shuffle(false, 0);
        let v = ids(&q);
        assert_eq!(&v[..2], &[1, 50], "play next still plays next");
        let p = v.iter().position(|&x| x == 60).unwrap();
        assert_eq!(v[p - 1], before_60, "an appended track follows what it followed");
        let known: Vec<i64> = v.iter().copied().filter(|x| *x < 50).collect();
        assert_eq!(known, (1..=6).collect::<Vec<_>>());
    }

    #[test]
    fn a_new_queue_under_shuffle_is_shuffled_and_remembered() {
        let mut q = Queue::default();
        q.replace((1..=3).map(t).collect(), 0);
        q.set_shuffle(true, 5);
        q.replace((11..=30).map(t).collect(), 0);
        assert!(q.shuffled());
        assert_eq!(q.current().unwrap().track.id, 11);
        assert_ne!(&ids(&q)[1..], &(12..=30).collect::<Vec<_>>()[..]);
        q.set_shuffle(false, 0);
        assert_eq!(ids(&q), (11..=30).collect::<Vec<_>>());
    }

    #[test]
    fn insert_and_move_keep_the_current_item() {
        let mut q = Queue::default();
        q.replace(vec![t(1), t(2), t(3), t(4)], 1);
        q.insert_at(0, vec![t(9)]);
        assert_eq!(ids(&q), vec![9, 1, 2, 3, 4]);
        assert_eq!(q.current().unwrap().track.id, 2);
        q.move_item(4, 0);
        assert_eq!(ids(&q), vec![4, 9, 1, 2, 3]);
        assert_eq!(q.current().unwrap().track.id, 2);
        q.move_item(3, 4); // the current item itself
        assert_eq!(ids(&q), vec![4, 9, 1, 3, 2]);
        assert_eq!(q.current().unwrap().track.id, 2);
        q.insert_at(99, vec![t(7)]);
        assert_eq!(*ids(&q).last().unwrap(), 7);
    }

    /// What is saved at exit and restored at start: the order, the current item, and
    /// under shuffle the order to go back to, including an item added while shuffled.
    #[test]
    fn snapshot_and_restore_keep_shuffle_and_its_original_order() {
        let mut q = Queue::default();
        q.replace((1..=8).map(t).collect(), 2);
        q.repeat = Repeat::All;
        q.set_shuffle(true, 42);
        q.insert_next(vec![t(50)]);
        let before = ids(&q);
        let snap = q.snapshot();
        let current = q.current_index();
        assert_eq!(snap.iter().filter(|(_, r)| r.is_none()).count(), 1, "only the added item has no original place");

        let tracks = |snap: &[(i64, Option<i64>)]| snap.iter().map(|&(id, r)| (t(id), r)).collect::<Vec<_>>();
        let mut back = Queue::restore(tracks(&snap), current, true, Repeat::All, 7);
        assert_eq!(ids(&back), before);
        assert_eq!(back.current_index(), current);
        assert!(back.shuffled());
        assert_eq!(back.repeat, Repeat::All);
        back.set_shuffle(false, 0);
        let mut expect = Queue::default();
        expect.replace((1..=8).map(t).collect(), 2);
        expect.insert_next(vec![t(50)]);
        assert_eq!(ids(&back), ids(&expect), "shuffle off restores the original order, the added item after its predecessor");
        assert_eq!(back.current().unwrap().track.id, 3);

        // A track gone at restore time is simply missing from both orders.
        let mut fewer = tracks(&snap);
        fewer.retain(|(t, _)| t.id != 5);
        let mut back = Queue::restore(fewer, Some(0), true, Repeat::Off, 1);
        back.set_shuffle(false, 0);
        assert_eq!(ids(&back), vec![1, 2, 3, 50, 4, 6, 7, 8]);

        // Not shuffled: as given.
        let plain = Queue::restore(vec![(t(1), None), (t(2), None)], Some(1), false, Repeat::One, 0);
        assert!(!plain.shuffled());
        assert_eq!((ids(&plain), plain.current_index(), plain.repeat), (vec![1, 2], Some(1), Repeat::One));
        assert!(Queue::restore(vec![], Some(3), false, Repeat::Off, 0).current().is_none());
    }

    #[test]
    fn update_tracks_takes_new_paths() {
        let mut q = Queue::default();
        q.replace(vec![t(1), t(2), t(3)], 1);
        let moved = |id: i64| (id != 3).then(|| Track { path: format!("moved/{id}.opus"), ..t(id) });
        assert_eq!(q.update_tracks(moved), (true, true));
        assert_eq!(q.items()[0].track.path, "moved/1.opus");
        assert_eq!(q.items()[2].track.path, "3.opus", "not found: left alone");
        assert_eq!(q.update_tracks(moved), (false, false));
    }

    #[test]
    fn revision_moves_with_changes_but_not_with_the_current_item() {
        let mut q = Queue::default();
        q.replace((1..=3).map(t).collect(), 0);
        let r = q.revision();
        q.skip_next();
        q.jump(2);
        q.set_current_uid(1);
        assert_eq!(q.revision(), r);
        q.append(vec![t(4)]);
        assert!(q.revision() > r);
    }

    #[test]
    fn shuffle_keeps_played_part_and_all_items() {
        let mut q = Queue::default();
        q.replace((1..=20).map(t).collect(), 4);
        q.shuffle_upcoming(12345);
        let v = ids(&q);
        assert_eq!(&v[..5], &[1, 2, 3, 4, 5]);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, (1..=20).collect::<Vec<_>>());
        assert_ne!(&v[5..], &(6..=20).collect::<Vec<_>>()[..]);
    }

    #[test]
    fn several_items_go_at_once() {
        let mut q = Queue::default();
        q.replace(vec![t(1), t(2), t(3), t(4), t(5)], 2);
        q.remove_many(&[0, 3]);
        assert_eq!(ids(&q), [2, 3, 5]);
        assert_eq!(q.current().unwrap().track.id, 3, "the current item stays current");
        // The current one among them: the first after it that stays.
        q.remove_many(&[1, 0, 9]);
        assert_eq!(ids(&q), [5]);
        assert_eq!(q.current().unwrap().track.id, 5);
        q.remove_many(&[0]);
        assert!(q.current().is_none());
        let mut q = Queue::default();
        q.replace(vec![t(1), t(2), t(3)], 2);
        q.remove_many(&[2, 1]);
        assert_eq!(ids(&q), [1]);
        assert!(q.current().is_none(), "nothing after the last one");
    }

    #[test]
    fn a_block_of_items_moves_together() {
        let mut q = Queue::default();
        q.replace(vec![t(1), t(2), t(3), t(4), t(5), t(6)], 3);
        assert_eq!(q.move_block(&[1, 3], 5), 3);
        assert_eq!(ids(&q), [1, 3, 5, 2, 4, 6]);
        assert_eq!(q.current().unwrap().track.id, 4, "the current item moved with the block");
        let r = q.revision();
        assert_eq!(q.move_block(&[3, 4], 3), 3);
        assert_eq!(q.revision(), r, "a move to where it is changes nothing");
    }
}
