//! A generic undo history. It only stores operations: the caller applies or reverts what
//! [`History::undo`] and [`History::redo`] hand back.

/// One undo step: a single op or a group that undoes as one.
#[derive(Clone, Debug)]
struct Step<Op>(Vec<Op>);

#[derive(Clone, Debug)]
pub struct History<Op> {
    done: Vec<Step<Op>>,
    undone: Vec<Step<Op>>,
    /// An open group collects pushes until `end_group` (nesting counts).
    group: Option<(Vec<Op>, u32)>,
    /// Oldest steps are dropped past this.
    pub limit: usize,
}

impl<Op> Default for History<Op> {
    fn default() -> Self {
        History { done: Vec::new(), undone: Vec::new(), group: None, limit: 500 }
    }
}

impl<Op> History<Op> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an op that was just applied. Clears the redo stack.
    pub fn push(&mut self, op: Op) {
        self.undone.clear();
        match &mut self.group {
            Some((ops, _)) => ops.push(op),
            None => self.commit(vec![op]),
        }
    }

    /// Ops pushed until the matching [`Self::end_group`] undo and redo as one step.
    pub fn begin_group(&mut self) {
        match &mut self.group {
            Some((_, depth)) => *depth += 1,
            None => self.group = Some((Vec::new(), 1)),
        }
    }

    pub fn end_group(&mut self) {
        let Some((_, depth)) = &mut self.group else { return };
        *depth -= 1;
        if *depth == 0 {
            let (ops, _) = self.group.take().unwrap();
            if !ops.is_empty() {
                self.commit(ops);
            }
        }
    }

    fn commit(&mut self, ops: Vec<Op>) {
        self.done.push(Step(ops));
        if self.done.len() > self.limit {
            let extra = self.done.len() - self.limit;
            self.done.drain(..extra);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// The last step's ops, newest first: revert them in this order.
    pub fn undo(&mut self) -> Option<Vec<Op>>
    where
        Op: Clone,
    {
        let step = self.done.pop()?;
        let ops = step.0.iter().rev().cloned().collect();
        self.undone.push(step);
        Some(ops)
    }

    /// The last undone step's ops, oldest first: apply them in this order.
    pub fn redo(&mut self) -> Option<Vec<Op>>
    where
        Op: Clone,
    {
        let step = self.undone.pop()?;
        let ops = step.0.clone();
        self.done.push(step);
        Some(ops)
    }

    /// Keeps only ops for which `keep` is true (e.g. drop ops of a draft that was sent).
    /// Steps left empty disappear.
    pub fn retain(&mut self, mut keep: impl FnMut(&Op) -> bool) {
        for stack in [&mut self.done, &mut self.undone] {
            for s in stack.iter_mut() {
                s.0.retain(&mut keep);
            }
            stack.retain(|s| !s.0.is_empty());
        }
        if let Some((ops, _)) = &mut self.group {
            ops.retain(keep);
        }
    }

    pub fn clear(&mut self) {
        self.done.clear();
        self.undone.clear();
        self.group = None;
    }
}

#[cfg(test)]
mod tests {
    use super::History;

    #[test]
    fn undo_redo_groups_limit_retain() {
        let mut h = History::new();
        h.push(1);
        h.begin_group();
        h.push(2);
        h.begin_group();
        h.push(3);
        h.end_group();
        h.end_group();
        assert_eq!(h.undo(), Some(vec![3, 2]), "a group undoes as one, newest first");
        assert_eq!(h.redo(), Some(vec![2, 3]));
        assert_eq!(h.undo(), Some(vec![3, 2]));
        h.push(4);
        assert!(!h.can_redo(), "a push clears redo");
        h.retain(|op| *op != 4);
        assert_eq!(h.undo(), Some(vec![1]), "retain dropped the empty step");
        assert_eq!(h.undo(), None);

        let mut h = History::new();
        h.limit = 3;
        for i in 0..5 {
            h.push(i);
        }
        let all: Vec<_> = std::iter::from_fn(|| h.undo()).flatten().collect();
        assert_eq!(all, vec![4, 3, 2]);
    }
}
