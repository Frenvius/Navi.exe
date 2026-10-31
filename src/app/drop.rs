use super::ids;
use crate::dnd::Spot;

pub fn pin_spot(
    tree: &super::navtree::NavTree,
    row: Option<usize>,
    y: f32,
    (height, margin): (f32, f32),
    offset: f32,
) -> Option<Spot> {
    let pins: Vec<_> = tree
        .rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            matches!(&row.reference, super::navtree::RowRef::Node { section, path }
            if *section == super::navtree::PINS && path.len() == 1)
            .then_some(index)
        })
        .collect();
    if let Some(row) = row {
        if tree
            .rows
            .get(row)
            .and_then(|row| tree.node(&row.reference))
            .is_some_and(|node| ids::same(&node.node.id, ids::HOME))
        {
            return Some(Spot::Pin {
                at: usize::MAX,
                line: -1,
                row: Some(row),
            });
        }
        let at = pins.iter().position(|&index| index == row)?;
        let top = margin + row as f32 * height + offset;
        if y - top < height / 3.0 {
            return Some(Spot::Pin {
                at,
                line: top as i32,
                row: None,
            });
        }
        if y - top > height * 2.0 / 3.0 {
            return Some(Spot::Pin {
                at: at + 1,
                line: (top + height) as i32,
                row: None,
            });
        }
    } else if let (Some(&first), Some(&last)) = (pins.first(), pins.last()) {
        let top = margin + first as f32 * height + offset;
        let bottom = margin + (last + 1) as f32 * height + offset;
        if y >= top - height && y < top {
            return Some(Spot::Pin {
                at: 0,
                line: top as i32,
                row: None,
            });
        }
        if y >= bottom && y < bottom + height {
            return Some(Spot::Pin {
                at: pins.len(),
                line: bottom as i32,
                row: None,
            });
        }
    }
    None
}

#[cfg(test)]
mod drop_tests {
    use super::*;
    #[test]
    fn pin_insertion_thirds_keep_the_center_a_folder_target() {
        let node = |id: &str, pinned| crate::shell::nav::NavNode {
            name: id.into(),
            id: id.into(),
            icon: 0,
            has_children: false,
            is_fs: true,
            is_pinned: pinned,
            group_id: None,
            namespace_clsid: None,
        };
        let mut tree = super::super::navtree::NavTree::default();
        tree.set_roots(
            vec![node(ids::HOME, false)],
            vec![node("C:\\Dummy A", true), node("C:\\Dummy B", true)],
            None,
        );
        let pin=tree.rows.iter().position(|row|matches!(&row.reference,super::super::navtree::RowRef::Node{section,path}if *section==super::super::navtree::PINS&&path==&[0])).unwrap();
        let top = 4.0 + pin as f32 * 24.0;
        for offset in 0..24 {
            let result = pin_spot(&tree, Some(pin), top + offset as f32, (24.0, 4.0), 0.0);
            match offset {
                0..=7 => assert_eq!(
                    result,
                    Some(Spot::Pin {
                        at: 0,
                        line: top as i32,
                        row: None
                    })
                ),
                8..=16 => assert!(result.is_none()),
                _ => assert_eq!(
                    result,
                    Some(Spot::Pin {
                        at: 1,
                        line: (top + 24.0) as i32,
                        row: None
                    })
                ),
            }
        }
        let home = tree
            .rows
            .iter()
            .position(|row| {
                tree.node(&row.reference)
                    .is_some_and(|node| ids::same(&node.node.id, ids::HOME))
            })
            .unwrap();
        assert_eq!(
            pin_spot(&tree, Some(home), 4.0, (24.0, 4.0), 0.0),
            Some(Spot::Pin {
                at: usize::MAX,
                line: -1,
                row: Some(home)
            })
        );
        assert_eq!(
            pin_spot(&tree, Some(pin), top - 48.0, (24.0, 4.0), -48.0),
            Some(Spot::Pin {
                at: 0,
                line: (top - 48.0) as i32,
                row: None
            })
        );
    }
}
