//! 置物架拖动的边界规则，坐标为面板局部逻辑点。
#[derive(Debug, PartialEq)]
pub(crate) enum Action {
    Finish,
    External,
    Inside { x: f32, edge: i8 },
    Wait,
}

pub(crate) fn action(pointer: Option<(f32, f32)>, down: bool, escape: bool) -> Action {
    if !down || escape { return Action::Finish; }
    let Some((x, y)) = pointer else { return Action::Wait; };
    // 只有面板内侧滚动；越过可见面板即交给系统拖放。
    if !(0.0..=419.0).contains(&x) || !(0.0..=140.0).contains(&y) {
        return Action::External;
    }
    let edge = if (28.0..=128.0).contains(&y) {
        if x < 75.0 { -1 } else if x > 344.0 { 1 } else { 0 }
    } else { 0 };
    Action::Inside { x, edge }
}

pub(crate) fn insert<T>(items: &mut Vec<T>, from: usize, to: usize) {
    if from != to {
        let item = items.remove(from);
        items.insert(to, item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_edges_scroll_inside_and_exit_outside() {
        for (x, edge) in [(1.0, -1), (418.0, 1)] {
            assert_eq!(action(Some((x, 65.0)), true, false), Action::Inside { x, edge });
        }
        for pos in [(-1.0, 65.0), (420.0, 65.0), (200.0, -1.0), (200.0, 141.0)] {
            assert_eq!(action(Some(pos), true, false), Action::External);
            assert_eq!(action(Some(pos), false, false), Action::Finish);
            assert_eq!(action(Some(pos), true, true), Action::Finish);
        }
    }
    #[test]
    fn returning_while_held_resumes_without_another_press() {
        for x in [-5.0, 424.0] {
            assert_eq!(action(Some((x, 65.0)), true, false), Action::External);
            assert_eq!(action(Some((200.0, 65.0)), true, false), Action::Inside { x: 200.0, edge: 0 });
        }
        assert_eq!(action(None, true, false), Action::Wait);
        assert_eq!(action(None, false, false), Action::Finish);
    }
    #[test]
    fn insertion_moves_first_to_last_and_back_without_swapping_neighbors() {
        let mut items: Vec<_> = (0..100).collect();
        for to in 1..100 { insert(&mut items, to - 1, to); }
        assert_eq!(items, (1..100).chain([0]).collect::<Vec<_>>());
        for to in (0..99).rev() { insert(&mut items, to + 1, to); }
        assert_eq!(items, (0..100).collect::<Vec<_>>());
    }
}
