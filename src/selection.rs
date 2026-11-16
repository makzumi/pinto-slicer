//! Which pieces are picked.
//!
//! Selection has rules of its own: a click replaces it, a shift-click toggles
//! one piece, a marquee takes everything it covers, and one piece is remembered
//! as primary so the single-piece panels and the handles know where to point.
//! Keeping those rules here means the window code never juggles indices.

/// The picked pieces, in the order they were picked, one of them primary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pieces: Vec<usize>,
    primary: Option<usize>,
}

impl Selection {
    pub fn none() -> Self {
        Self::default()
    }

    /// Just this piece.
    pub fn single(index: usize) -> Self {
        Self {
            pieces: vec![index],
            primary: Some(index),
        }
    }

    pub fn pieces(&self) -> &[usize] {
        &self.pieces
    }

    pub fn primary(&self) -> Option<usize> {
        self.primary
    }

    pub fn contains(&self, index: usize) -> bool {
        self.pieces.contains(&index)
    }

    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    pub fn clear(&mut self) {
        self.pieces.clear();
        self.primary = None;
    }

    /// Take everything there is.
    pub fn all(&mut self, count: usize) {
        self.pieces = (0..count).collect();
        self.primary = count.checked_sub(1);
    }

    /// Exactly these pieces, replacing whatever was picked.
    pub fn only(&mut self, indices: impl IntoIterator<Item = usize>) {
        self.pieces = indices.into_iter().collect();
        self.primary = self.pieces.last().copied();
    }

    /// A click: `hit` is the piece under the pointer, `additive` says the user
    /// was holding shift or control. Clicking empty space clears the selection
    /// unless the click is additive.
    pub fn click(&mut self, hit: Option<usize>, additive: bool) {
        match (hit, additive) {
            (None, false) => self.clear(),
            (None, true) => {}
            (Some(i), false) => *self = Self::single(i),
            (Some(i), true) => {
                if self.contains(i) {
                    self.pieces.retain(|&p| p != i);
                    self.primary = self.pieces.last().copied();
                } else {
                    self.pieces.push(i);
                    self.primary = Some(i);
                }
            }
        }
    }

    /// The pieces a marquee covered. Additive keeps what was already picked.
    pub fn covered(&mut self, hits: &[usize], additive: bool) {
        let mut next = if additive {
            self.pieces.clone()
        } else {
            Vec::new()
        };
        for &i in hits {
            if !next.contains(&i) {
                next.push(i);
            }
        }
        self.primary = next.last().copied();
        self.pieces = next;
    }

    /// Drop anything at or past `count` pieces: used after the plate changes.
    pub fn prune(&mut self, count: usize) {
        self.pieces.retain(|&p| p < count);
        self.primary = self.pieces.last().copied();
    }

    /// Follow the pieces through a removal. `map[old]` is the piece's new index,
    /// or `None` when it went away.
    pub fn remap(&mut self, map: &[Option<usize>]) {
        let before = self.primary;
        self.pieces = self
            .pieces
            .iter()
            .filter_map(|&old| map.get(old).copied().flatten())
            .collect();
        self.primary = before
            .and_then(|old| map.get(old).copied().flatten())
            .or_else(|| self.pieces.last().copied());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_replaces_the_selection() {
        let mut s = Selection::single(1);
        s.click(Some(4), false);
        assert_eq!(s.pieces(), &[4]);
        assert_eq!(s.primary(), Some(4));
    }

    #[test]
    fn clicking_empty_space_clears_it() {
        let mut s = Selection::single(2);
        s.click(None, false);
        assert!(s.is_empty());
        assert_eq!(s.primary(), None);
    }

    #[test]
    fn a_shift_click_adds_and_removes() {
        let mut s = Selection::none();
        s.click(Some(0), true);
        s.click(Some(3), true);
        assert_eq!(s.pieces(), &[0, 3]);
        assert_eq!(s.primary(), Some(3), "the last one added is primary");
        s.click(Some(3), true);
        assert_eq!(s.pieces(), &[0]);
        assert_eq!(s.primary(), Some(0), "primary falls back to what is left");
        s.click(None, true);
        assert_eq!(s.pieces(), &[0], "a shift click on nothing keeps the rest");
    }

    #[test]
    fn a_marquee_replaces_unless_additive() {
        let mut s = Selection::single(9);
        s.covered(&[1, 2], false);
        assert_eq!(s.pieces(), &[1, 2]);
        s.covered(&[2, 5], true);
        assert_eq!(
            s.pieces(),
            &[1, 2, 5],
            "no duplicates when the boxes overlap"
        );
        s.covered(&[], false);
        assert!(s.is_empty(), "an empty box picks nothing");
    }

    #[test]
    fn only_replaces_what_was_picked() {
        let mut s = Selection::single(0);
        s.only(3..5);
        assert_eq!(s.pieces(), &[3, 4]);
        assert_eq!(s.primary(), Some(4));
    }

    #[test]
    fn pruning_and_removal_keep_the_indices_sane() {
        let mut s = Selection::none();
        s.covered(&[0, 2], false);
        s.prune(2);
        assert_eq!(s.pieces(), &[0]);

        // piece 0 removed, so 1 becomes 0 and 2 becomes 1
        let mut s = Selection::single(2);
        s.remap(&[None, Some(0), Some(1)]);
        assert_eq!(s.pieces(), &[1]);
        assert_eq!(s.primary(), Some(1));
    }

    #[test]
    fn taking_everything_picks_the_last_as_primary() {
        let mut s = Selection::none();
        s.all(3);
        assert_eq!(s.len(), 3);
        assert_eq!(s.primary(), Some(2));
        let mut empty = Selection::none();
        empty.all(0);
        assert!(empty.is_empty());
        assert_eq!(empty.primary(), None);
    }
}
