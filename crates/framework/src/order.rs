// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Load ordering: the constraint vocabulary of plugin descriptors and the sort that resolves it.

use std::cmp::Reverse;
use std::fmt;

/// Names every other item in a `before` or `after` list.
pub const ALL: &str = "*";

/// What one item asks of the order. A name that is not present is reported and ignored; `ALL`
/// in `before` ignores the other items that also say `before: ALL`, likewise for `after`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Constraints {
    pub before: Vec<String>,
    pub after: Vec<String>,
    /// Decides between items whose constraints are both satisfied: higher loads first.
    pub priority: i32,
}

impl fmt::Display for Constraints {
    /// `before [a, b], after [*], priority 2`, leaving out what is at its default.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        for (what, list) in [("before", &self.before), ("after", &self.after)] {
            if !list.is_empty() {
                parts.push(format!("{what} [{}]", list.join(", ")));
            }
        }
        if self.priority != 0 {
            parts.push(format!("priority {}", self.priority));
        }
        f.write_str(&parts.join(", "))
    }
}

/// The result of [`sort`]: indices into its input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Order {
    /// Every item outside a cycle, in the order to load.
    pub sorted: Vec<usize>,
    /// Names no item has, with the index of the item that asked.
    pub missing: Vec<(usize, String)>,
    /// Items whose `before`/`after` constraints contradict each other, left out of `sorted`.
    pub cycle: Vec<usize>,
}

/// Orders `items` (name and constraints, in their fallback order) by their constraints first,
/// priority second and position last.
#[must_use]
pub fn sort(items: &[(&str, &Constraints)]) -> Order {
    let n = items.len();
    let mut first = vec![vec![false; n]; n];
    let mut missing = Vec::new();
    for (i, (_, c)) in items.iter().enumerate() {
        for (list, forward) in [(&c.before, true), (&c.after, false)] {
            for name in list {
                let same_all = |c: &Constraints| if forward { &c.before } else { &c.after }.iter().any(|x| x == ALL);
                let targets: Vec<usize> = (0..n).filter(|&j| j != i && if name == ALL { !same_all(items[j].1) } else { items[j].0 == name }).collect();
                if targets.is_empty() && name != ALL {
                    missing.push((i, name.clone()));
                }
                for j in targets {
                    let (a, b) = if forward { (i, j) } else { (j, i) };
                    first[a][b] = true;
                }
            }
        }
    }
    let mut live = vec![true; n];
    loop {
        let dead: Vec<usize> = (0..n).filter(|&i| live[i] && !((0..n).any(|j| live[j] && first[j][i]) && (0..n).any(|j| live[j] && first[i][j]))).collect();
        if dead.is_empty() {
            break;
        }
        for i in dead {
            live[i] = false;
        }
    }
    let cycle: Vec<usize> = (0..n).filter(|&i| live[i]).collect();
    let mut placed = live;
    let mut sorted = Vec::with_capacity(n - cycle.len());
    while sorted.len() < n - cycle.len() {
        let next = (0..n).filter(|&i| !placed[i] && !(0..n).any(|j| !placed[j] && first[j][i])).max_by_key(|&i| (items[i].1.priority, Reverse(i))).expect("the remaining items are acyclic");
        placed[next] = true;
        sorted.push(next);
    }
    Order { sorted, missing, cycle }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names<'a>(order: &Order, items: &[(&'a str, &Constraints)]) -> Vec<&'a str> {
        order.sorted.iter().map(|&i| items[i].0).collect()
    }

    fn c(before: &[&str], after: &[&str], priority: i32) -> Constraints {
        Constraints { before: before.iter().map(|s| (*s).to_owned()).collect(), after: after.iter().map(|s| (*s).to_owned()).collect(), priority }
    }

    #[test]
    fn keeps_position_without_constraints() {
        let none = Constraints::default();
        let items = [("c", &none), ("a", &none), ("b", &none)];
        let order = sort(&items);
        assert_eq!(names(&order, &items), ["c", "a", "b"]);
        assert_eq!(order, Order { sorted: vec![0, 1, 2], ..Order::default() });
    }

    #[test]
    fn higher_priority_loads_first() {
        let (low, high, none) = (c(&[], &[], -1), c(&[], &[], 5), Constraints::default());
        let items = [("a", &low), ("b", &none), ("c", &high)];
        assert_eq!(names(&sort(&items), &items), ["c", "b", "a"]);
    }

    #[test]
    fn constraints_beat_priority_and_position() {
        let (before_a, after_d, none) = (c(&["a"], &[], 0), c(&[], &["d"], 9), Constraints::default());
        let items = [("a", &none), ("b", &before_a), ("c", &after_d), ("d", &none)];
        let order = sort(&items);
        assert_eq!(names(&order, &items), ["b", "a", "d", "c"]);
        assert!(order.missing.is_empty() && order.cycle.is_empty());
    }

    #[test]
    fn all_means_every_other_item() {
        let (last, first, none) = (c(&[], &[ALL], 9), c(&[ALL], &[], 0), Constraints::default());
        let items = [("a", &last), ("b", &none), ("c", &first), ("d", &last)];
        assert_eq!(names(&sort(&items), &items), ["c", "b", "a", "d"]);
    }

    #[test]
    fn missing_name_is_reported_and_ignored() {
        let (asks, none) = (c(&[], &["nowhere"], 1), Constraints::default());
        let items = [("a", &none), ("b", &asks)];
        let order = sort(&items);
        assert_eq!(names(&order, &items), ["b", "a"]);
        assert_eq!(order.missing, [(1, "nowhere".to_owned())]);
    }

    #[test]
    fn cycle_is_refused_and_the_rest_load() {
        let (after_b, after_a, after_all, none) = (c(&[], &["b"], 0), c(&[], &["a"], 0), c(&[], &[ALL], 0), Constraints::default());
        let items = [("a", &after_b), ("b", &after_a), ("c", &after_all), ("d", &none)];
        let order = sort(&items);
        assert_eq!(order.cycle, [0, 1]);
        assert_eq!(names(&order, &items), ["d", "c"]);
    }
}
