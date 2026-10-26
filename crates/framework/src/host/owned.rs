//! A list of callbacks tagged with the plugin that registered each, called without a lock held
//! and removable by owner with every call inside the removed entries drained first.

use std::sync::{Arc, Mutex, PoisonError, RwLock};

struct Entry<T> {
    owner: &'static str,
    callback: RwLock<Option<T>>,
}

/// The entries as the calls walk them; a change publishes a new list.
type List<T> = Arc<Vec<Arc<Entry<T>>>>;

pub(crate) struct Owned<T> {
    entries: Mutex<Option<List<T>>>,
}

impl<T> Owned<T> {
    pub(crate) const fn new() -> Self {
        Self { entries: Mutex::new(None) }
    }

    fn snapshot(&self) -> Option<List<T>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub(crate) fn add(&self, owner: &'static str, callback: T) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let mut next: Vec<Arc<Entry<T>>> = entries.as_deref().cloned().unwrap_or_default();
        next.push(Arc::new(Entry { owner, callback: RwLock::new(Some(callback)) }));
        *entries = Some(Arc::new(next));
    }

    /// Calls `f` with every callback in registration order until it returns false. Registering
    /// from inside `f` is fine; removing is not (it would wait for this call).
    pub(crate) fn each(&self, mut f: impl FnMut(&T) -> bool) {
        for entry in self.snapshot().iter().flat_map(|v| v.iter()) {
            let callback = entry.callback.read().unwrap_or_else(PoisonError::into_inner);
            if let Some(callback) = callback.as_ref()
                && !f(callback)
            {
                return;
            }
        }
    }

    /// Like [`each`](Self::each) with exclusive access to each callback, for `FnMut`s.
    pub(crate) fn each_mut(&self, mut f: impl FnMut(&mut T)) {
        for entry in self.snapshot().iter().flat_map(|v| v.iter()) {
            let mut callback = entry.callback.write().unwrap_or_else(PoisonError::into_inner);
            if let Some(callback) = callback.as_mut() {
                f(callback);
            }
        }
    }

    /// Removes every callback of `owner` and drops each once no call is inside it; returns how many.
    pub(crate) fn remove_owner(&self, owner: &str) -> usize {
        let removed: Vec<Arc<Entry<T>>> = {
            let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            let all: Vec<Arc<Entry<T>>> = entries.as_deref().cloned().unwrap_or_default();
            let (gone, kept): (Vec<_>, Vec<_>) = all.into_iter().partition(|e| e.owner == owner);
            *entries = Some(Arc::new(kept));
            gone
        };
        for entry in &removed {
            drop(entry.callback.write().unwrap_or_else(PoisonError::into_inner).take());
        }
        removed.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_in_order_and_removes_by_owner() {
        let owned: Owned<u32> = Owned::new();
        owned.add("a", 1);
        owned.add("b", 2);
        owned.add("a", 3);
        let mut seen = Vec::new();
        owned.each(|v| {
            seen.push(*v);
            *v != 2
        });
        assert_eq!(seen, [1, 2]);
        assert_eq!(owned.remove_owner("a"), 2);
        assert_eq!(owned.remove_owner("a"), 0);
        seen.clear();
        owned.each_mut(|v| seen.push(*v));
        assert_eq!(seen, [2]);
    }

    #[test]
    fn registering_from_inside_a_call_is_allowed() {
        static OWNED: Owned<u32> = Owned::new();
        OWNED.add("a", 1);
        let mut calls = 0;
        OWNED.each(|_| {
            OWNED.add("b", 2);
            calls += 1;
            true
        });
        assert_eq!(calls, 1);
        assert_eq!(OWNED.remove_owner("b"), 1);
    }
}
