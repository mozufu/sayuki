//! Per-canvas MRU focus stack.
//!
//! The stack is ordered oldest-to-newest; the tail (`last`) is the focused
//! member. Operations are generic over the element identity type so the
//! invariants can be unit-tested without a real Smithay `Window`.

/// Direction for [`cycle`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CycleDirection {
    /// Move focus toward the front of the stack (older windows), wrapping.
    Forward,
    /// Move focus toward the back of the stack (newer windows), wrapping.
    Backward,
}

/// Focus `item`, moving it to the tail. If it is already present it is moved
/// rather than duplicated, preserving the single-occurrence invariant.
pub fn focus<T: PartialEq>(stack: &mut Vec<T>, item: T) {
    stack.retain(|existing| existing != &item);
    stack.push(item);
}

/// Remove `item` from the stack. Returns `true` when the removed item was the
/// focused (tail) element, so the caller can re-focus the new tail.
pub fn remove<T: PartialEq>(stack: &mut Vec<T>, item: &T) -> bool {
    let Some(position) = stack.iter().position(|existing| existing == item) else {
        return false;
    };
    let was_focused = position + 1 == stack.len();
    stack.remove(position);
    was_focused
}

/// Rotate focus through the stack. The element that becomes focused is moved to
/// the tail; with fewer than two members this is a no-op. Repeated calls visit
/// every member in turn and a `Forward` followed by a `Backward` is the
/// identity, so the gesture is reversible.
pub fn cycle<T>(stack: &mut [T], direction: CycleDirection) {
    if stack.len() < 2 {
        return;
    }
    match direction {
        CycleDirection::Forward => stack.rotate_left(1),
        CycleDirection::Backward => stack.rotate_right(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focused(stack: &[i32]) -> Option<i32> {
        stack.last().copied()
    }

    #[test]
    fn focus_appends_new_and_dedups_existing() {
        let mut stack = Vec::new();
        focus(&mut stack, 1);
        focus(&mut stack, 2);
        focus(&mut stack, 3);
        assert_eq!(stack, [1, 2, 3]);

        // Re-focusing an existing member moves it to the tail without duplicating.
        focus(&mut stack, 1);
        assert_eq!(stack, [2, 3, 1]);
        assert_eq!(focused(&stack), Some(1));
        // Single-occurrence invariant.
        assert_eq!(stack.iter().filter(|item| **item == 1).count(), 1);
    }

    #[test]
    fn remove_reports_whether_focused_and_drops_member() {
        let mut stack = vec![1, 2, 3];
        // Removing the focused tail reports true.
        assert!(remove(&mut stack, &3));
        assert_eq!(stack, [1, 2]);
        // Removing a non-tail member reports false (do not steal focus).
        assert!(!remove(&mut stack, &1));
        assert_eq!(stack, [2]);
        // Removing an absent member is a no-op reporting false.
        assert!(!remove(&mut stack, &9));
        assert_eq!(stack, [2]);
    }

    #[test]
    fn cycle_visits_every_member_and_is_reversible() {
        let mut stack = vec![1, 2, 3];
        // Forward cycles the focused window front-to-back through all members.
        cycle(&mut stack, CycleDirection::Forward);
        assert_eq!(focused(&stack), Some(1));
        cycle(&mut stack, CycleDirection::Forward);
        assert_eq!(focused(&stack), Some(2));
        cycle(&mut stack, CycleDirection::Forward);
        assert_eq!(focused(&stack), Some(3));

        // Forward then Backward returns to the starting arrangement.
        cycle(&mut stack, CycleDirection::Forward);
        cycle(&mut stack, CycleDirection::Backward);
        assert_eq!(stack, [1, 2, 3]);
    }

    #[test]
    fn cycle_is_noop_below_two_members() {
        let mut empty: Vec<i32> = Vec::new();
        cycle(&mut empty, CycleDirection::Forward);
        assert!(empty.is_empty());

        let mut single = vec![7];
        cycle(&mut single, CycleDirection::Backward);
        assert_eq!(single, [7]);
    }
}
