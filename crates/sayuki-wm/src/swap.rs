//! Window swap: a discrete reorder distinct from snap. Drop-onto-window and the
//! directional/MRU keybinds exchange two windows' canvas rectangles (position
//! and size).

use smithay::utils::{Logical, Point, Rectangle};

/// A direction for [`SwapTarget::Direction`] / directional focus selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// What a `SwapWindow` action swaps the focused window with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwapTarget {
    /// The geometrically nearest window in a direction (by center).
    Direction(Direction),
    /// The next window along the MRU stack.
    Next,
    /// The previous window along the MRU stack.
    Prev,
}

/// The two rectangles after a swap: each window takes the other's position and
/// size. Returned as `(first, second)` matching the input order.
pub fn exchange(
    first: Rectangle<i32, Logical>,
    second: Rectangle<i32, Logical>,
) -> (Rectangle<i32, Logical>, Rectangle<i32, Logical>) {
    (second, first)
}

fn center(rect: Rectangle<i32, Logical>) -> Point<i32, Logical> {
    rect.loc + Point::from((rect.size.w / 2, rect.size.h / 2))
}

/// Pick the index of the candidate whose center is nearest to `focused`'s
/// center within the half-plane of `direction`. Candidates not in the direction
/// are ignored; ties break toward the smaller squared distance.
pub fn nearest_in_direction(
    focused: Rectangle<i32, Logical>,
    candidates: &[(usize, Rectangle<i32, Logical>)],
    direction: Direction,
) -> Option<usize> {
    let origin = center(focused);
    let mut best: Option<(usize, i64)> = None;

    for (id, rect) in candidates {
        let target = center(*rect);
        let dx = i64::from(target.x - origin.x);
        let dy = i64::from(target.y - origin.y);

        let in_direction = match direction {
            Direction::Left => dx < 0,
            Direction::Right => dx > 0,
            Direction::Up => dy < 0,
            Direction::Down => dy > 0,
        };
        if !in_direction {
            continue;
        }

        let distance = dx * dx + dy * dy;
        if best.is_none_or(|(_, best_distance)| distance < best_distance) {
            best = Some((*id, distance));
        }
    }

    best.map(|(id, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::utils::Size;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((x, y)), Size::from((w, h)))
    }

    #[test]
    fn exchange_swaps_position_and_size() {
        let a = rect(0, 0, 100, 100);
        let b = rect(500, 300, 200, 150);
        let (new_a, new_b) = exchange(a, b);
        assert_eq!(new_a, b);
        assert_eq!(new_b, a);
    }

    #[test]
    fn nearest_to_the_right_picks_closest_in_half_plane() {
        let focused = rect(0, 0, 100, 100);
        let candidates = [
            (1, rect(200, 0, 100, 100)),  // right, near
            (2, rect(800, 0, 100, 100)),  // right, far
            (3, rect(-300, 0, 100, 100)), // left, ignored
        ];
        assert_eq!(
            nearest_in_direction(focused, &candidates, Direction::Right),
            Some(1)
        );
        assert_eq!(
            nearest_in_direction(focused, &candidates, Direction::Left),
            Some(3)
        );
    }

    #[test]
    fn nearest_returns_none_when_nothing_in_direction() {
        let focused = rect(0, 0, 100, 100);
        let candidates = [(1, rect(200, 0, 100, 100))];
        assert_eq!(
            nearest_in_direction(focused, &candidates, Direction::Up),
            None
        );
    }

    #[test]
    fn nearest_uses_vertical_axis_for_up_down() {
        let focused = rect(0, 0, 100, 100);
        let candidates = [
            (1, rect(0, -400, 100, 100)), // up, far
            (2, rect(0, -150, 100, 100)), // up, near
            (3, rect(0, 300, 100, 100)),  // down
        ];
        assert_eq!(
            nearest_in_direction(focused, &candidates, Direction::Up),
            Some(2)
        );
        assert_eq!(
            nearest_in_direction(focused, &candidates, Direction::Down),
            Some(3)
        );
    }
}
