use crate::app::ids;
use crate::shell::nav::NavNode;

const CLSID_HOME: &str = "F874310E-B6B7-47DC";
const CLSID_GALLERY: &str = "E88865EA-0E1C-4E20";
const CLSID_ONEDRIVE: &str = "018D5C66-4533-4307";
const CLSID_THIS_PC: &str = "20D04FE0-3AEA-1069";
const CLSID_LIBRARIES: &str = "031E4825-7B94-4DC3";
const CLSID_NETWORK: &str = "F02C1A0D-BE21-4350";

/// Explorer groups the namespace roots as: Home, Gallery and cloud drives / pinned folders /
/// This PC, Network and every other root (WSL, removable drives...).
fn rank(node: &NavNode) -> u8 {
    let id = node.id.to_uppercase();
    if id.contains(CLSID_HOME) {
        0
    } else if id.contains(CLSID_GALLERY) {
        1
    } else if id.contains(CLSID_ONEDRIVE) || (node.is_fs && !ids::is_drive_root(&node.id)) {
        2
    } else if id.contains(CLSID_THIS_PC) {
        5
    } else if id.contains(CLSID_LIBRARIES) {
        6
    } else if id.contains(CLSID_NETWORK) {
        7
    } else if ids::is_drive_root(&node.id) {
        8
    } else {
        9
    }
}

#[derive(Clone)]
pub struct TreeNode {
    pub node: NavNode,
    pub expanded: bool,
    pub children: Option<Vec<TreeNode>>,
}

impl TreeNode {
    fn new(node: NavNode) -> TreeNode {
        TreeNode {
            node,
            expanded: false,
            children: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RowRef {
    Separator,
    Node { section: usize, path: Vec<usize> },
}

/// Whether `inner` is a row under `outer` (deeper in the same subtree).
pub fn is_inside(inner: &RowRef, outer: &RowRef) -> bool {
    match (inner, outer) {
        (
            RowRef::Node {
                section: a,
                path: p,
            },
            RowRef::Node {
                section: b,
                path: q,
            },
        ) => a == b && p.len() > q.len() && p.starts_with(q),
        _ => false,
    }
}

pub struct Row {
    pub depth: i32,
    pub reference: RowRef,
}

#[derive(Default)]
pub struct NavTree {
    pub sections: Vec<Vec<TreeNode>>,
    pub rows: Vec<Row>,
}

pub const PINS: usize = 1;

#[derive(Clone, Copy)]
pub enum Follow {
    Ancestors,
    Current,
}

pub struct FollowStep {
    pub row: usize,
    pub open: bool,
    pub loaded: bool,
}

fn unique_roots(roots: Vec<NavNode>) -> Vec<NavNode> {
    let mut seen = std::collections::HashSet::new();
    let mut roots: Vec<_> = roots
        .into_iter()
        .rev()
        .filter(|node| seen.insert(node.id.to_ascii_lowercase()))
        .collect();
    roots.reverse();
    roots
}

impl NavTree {
    pub fn follow(&self, chain: &[String], mode: Follow) -> Option<FollowStep> {
        let desktop = self
            .sections
            .first()
            .and_then(|section| section.first())
            .is_some_and(|node| node.node.id.is_empty());
        let row = self.selected_row(chain).or_else(|| desktop.then_some(0))?;
        let node = self.node(&self.rows.get(row)?.reference)?;
        let current = chain.last().is_some_and(|id| ids::same(id, &node.node.id));
        Some(FollowStep {
            row,
            open: !node.expanded
                && node.node.has_children
                && (matches!(mode, Follow::Current) || !current),
            loaded: node.children.is_some(),
        })
    }

    pub fn typeahead(&self, current: usize, prefix: &str) -> Option<usize> {
        let single = prefix
            .chars()
            .all(|character| Some(character) == prefix.chars().next());
        let (start, needle) = if single {
            (current + 1, prefix.chars().take(1).collect::<String>())
        } else {
            (current, prefix.to_owned())
        };
        let count = self.rows.len();
        (0..count)
            .map(|offset| (start + offset) % count)
            .find(|&row| {
                self.node(&self.rows[row].reference)
                    .is_some_and(|node| node.node.name.to_lowercase().starts_with(&needle))
            })
    }

    pub fn step(&self, row: usize, step: isize) -> Option<usize> {
        let mut at = row as isize + step;
        while at >= 0 && at < self.rows.len() as isize {
            if self.node(&self.rows[at as usize].reference).is_some() {
                return Some(at as usize);
            }
            at += step.signum();
        }
        None
    }

    pub fn parent(&self, row: usize) -> Option<usize> {
        let depth = self.rows.get(row)?.depth;
        (0..row).rev().find(|&index| {
            self.rows[index].depth < depth && self.node(&self.rows[index].reference).is_some()
        })
    }

    pub fn page_target(&self, current: usize, direction: isize, page: usize) -> usize {
        if self.rows.is_empty() {
            return current;
        }
        let target = (current as isize + direction * page.saturating_sub(1).max(1) as isize)
            .clamp(0, self.rows.len() as isize - 1) as usize;
        if self.node(&self.rows[target].reference).is_some() {
            target
        } else {
            self.step(target, direction).unwrap_or(current)
        }
    }

    pub fn subtree_end(&self, row: usize) -> usize {
        let Some(parent) = self.rows.get(row) else {
            return row;
        };
        let mut end = row;
        while let Some(next) = self.rows.get(end + 1) {
            if next.reference == RowRef::Separator || next.depth <= parent.depth {
                break;
            }
            end += 1;
        }
        end
    }

    pub fn picked_row(
        &self,
        pick: Option<&Pick>,
        path: &str,
        crumbs: &[crate::shell::listing::Crumb],
    ) -> Option<usize> {
        let picked = pick
            .filter(|pick| ids::same(&pick.at, path) || ids::same(&pick.id, path))
            .and_then(|pick| {
                self.rows.iter().position(|row| {
                    row.reference == pick.reference
                        && self
                            .node(&row.reference)
                            .is_some_and(|node| ids::same(&node.node.id, &pick.id))
                })
            });
        picked.or_else(|| {
            let mut chain: Vec<String> = crumbs.iter().map(|crumb| crumb.id.clone()).collect();
            if let Some(root) = crumbs.first().and_then(|crumb| crumb.nav_root.as_ref()) {
                if let Some(first) = chain.first_mut() {
                    *first = root.clone();
                }
                chain.insert(0, ids::HOME.into());
            }
            self.selected_row(&chain)
        })
    }

    pub fn set_roots(&mut self, roots: Vec<NavNode>, pins: Vec<NavNode>, desktop: Option<NavNode>) {
        let roots = if desktop.is_some() {
            unique_roots(roots)
        } else {
            roots
        };
        let mut home: Vec<NavNode> = roots.iter().filter(|n| rank(n) <= 2).cloned().collect();
        let mut devices: Vec<NavNode> = roots.iter().filter(|n| rank(n) > 2).cloned().collect();
        home.sort_by_key(rank);
        devices.sort_by_key(rank);
        for node in home.iter_mut() {
            if rank(node) == 0 {
                node.has_children = false;
            }
        }
        let pins: Vec<NavNode> = pins.into_iter().filter(|p| p.is_pinned).collect();

        let old = std::mem::take(&mut self.sections);
        self.sections = if let Some(desktop) = desktop {
            vec![
                vec![TreeNode {
                    node: desktop,
                    expanded: true,
                    children: Some(roots.into_iter().map(TreeNode::new).collect()),
                }],
                Vec::new(),
                Vec::new(),
            ]
        } else {
            vec![
                home.into_iter().map(TreeNode::new).collect(),
                pins.into_iter().map(TreeNode::new).collect(),
                devices.into_iter().map(TreeNode::new).collect(),
            ]
        };
        fn collect(nodes: &[TreeNode], kept: &mut std::collections::HashMap<String, TreeNode>) {
            for node in nodes {
                if node.expanded || node.node.id.is_empty() {
                    kept.insert(node.node.id.to_ascii_lowercase(), node.clone());
                }
                if let Some(children) = &node.children {
                    collect(children, kept)
                }
            }
        }
        fn restore(nodes: &mut [TreeNode], kept: &std::collections::HashMap<String, TreeNode>) {
            for node in nodes {
                if let Some(old) = kept.get(&node.node.id.to_ascii_lowercase()) {
                    node.expanded = old.expanded;
                    if node.children.is_none() {
                        node.children = old.children.clone();
                    }
                }
                if let Some(children) = &mut node.children {
                    restore(children, kept)
                }
            }
        }
        let mut kept = std::collections::HashMap::new();
        for section in &old {
            collect(section, &mut kept)
        }
        for section in &mut self.sections {
            restore(section, &kept)
        }
        self.flatten();
    }

    pub fn replace_children(&mut self, id: &str, children: Vec<NavNode>) -> bool {
        let children = if id.is_empty() {
            unique_roots(children)
        } else {
            children
        };
        fn replace(nodes: &mut [TreeNode], id: &str, children: &[NavNode]) -> bool {
            let mut found = false;
            for node in nodes {
                if ids::same(&node.node.id, id) {
                    let mut old: std::collections::HashMap<_, _> = node
                        .children
                        .take()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|child| (child.node.id.to_ascii_lowercase(), child))
                        .collect();
                    node.children = Some(
                        children
                            .iter()
                            .map(|child| {
                                let previous = old.remove(&child.id.to_ascii_lowercase());
                                TreeNode {
                                    node: child.clone(),
                                    expanded: previous.as_ref().is_some_and(|node| node.expanded),
                                    children: previous.and_then(|node| node.children),
                                }
                            })
                            .collect(),
                    );
                    if children.is_empty() {
                        node.node.has_children = false;
                        node.expanded = false;
                    }
                    found = true;
                } else if let Some(descendants) = &mut node.children {
                    found |= replace(descendants, id, children);
                }
            }
            found
        }
        let mut found = false;
        for section in &mut self.sections {
            found |= replace(section, id, &children);
        }
        if found {
            self.flatten();
        }
        found
    }

    pub fn node(&self, reference: &RowRef) -> Option<&TreeNode> {
        let RowRef::Node { section, path } = reference else {
            return None;
        };
        let (last, parents) = path.split_last()?;
        let mut list = self.sections.get(*section)?;
        for &i in parents {
            list = list.get(i)?.children.as_ref()?;
        }
        list.get(*last)
    }

    pub fn node_mut(&mut self, reference: &RowRef) -> Option<&mut TreeNode> {
        let RowRef::Node { section, path } = reference else {
            return None;
        };
        let (last, parents) = path.split_last()?;
        let mut list = self.sections.get_mut(*section)?;
        for &i in parents {
            list = list.get_mut(i)?.children.as_mut()?;
        }
        list.get_mut(*last)
    }

    pub fn flatten(&mut self) {
        fn walk(
            nodes: &[TreeNode],
            section: usize,
            prefix: &mut Vec<usize>,
            depth: i32,
            rows: &mut Vec<Row>,
        ) {
            for (i, node) in nodes.iter().enumerate() {
                prefix.push(i);
                rows.push(Row {
                    depth,
                    reference: RowRef::Node {
                        section,
                        path: prefix.clone(),
                    },
                });
                if node.expanded {
                    if let Some(children) = &node.children {
                        walk(children, section, prefix, depth + 1, rows);
                    }
                }
                prefix.pop();
            }
        }
        let mut rows = Vec::new();
        let mut first = true;
        for (s, nodes) in self.sections.iter().enumerate() {
            if nodes.is_empty() {
                continue;
            }
            if !first {
                rows.push(Row {
                    depth: 0,
                    reference: RowRef::Separator,
                });
            }
            first = false;
            walk(nodes, s, &mut Vec::new(), 0, &mut rows);
        }
        self.rows = rows;
    }

    /// The row Explorer highlights for a location: the deepest visible ancestor outside the
    /// pinned group (pins only match exactly).
    pub fn selected_row(&self, chain: &[String]) -> Option<usize> {
        let current = chain.last()?;
        let mut best: Option<(usize, usize)> = None;
        for (row_index, row) in self.rows.iter().enumerate() {
            let RowRef::Node { section, .. } = &row.reference else {
                continue;
            };
            let Some(node) = self.node(&row.reference) else {
                continue;
            };
            if *section == PINS {
                continue;
            }
            if let Some(depth) = chain.iter().position(|id| ids::same(id, &node.node.id)) {
                if best.map(|(d, _)| depth > d).unwrap_or(true) {
                    best = Some((depth, row_index));
                }
            }
        }
        if best.is_none() {
            return self.rows.iter().position(|row| {
                self.node(&row.reference)
                    .map(|n| ids::same(&n.node.id, current))
                    .unwrap_or(false)
            });
        }
        best.map(|(_, row)| row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(id: &str) -> NavNode {
        NavNode {
            name: id.into(),
            id: id.into(),
            icon: 0,
            has_children: true,
            is_fs: false,
            is_pinned: false,
        }
    }

    #[test]
    fn all_folders_keeps_the_last_namespace_identity_at_its_shell_position() {
        let mut alias = node("C:\\Known");
        alias.name = "alias".into();
        let canonical = node("c:\\known");
        let mut tree = NavTree::default();
        tree.set_roots(
            vec![alias, node(ids::HOME), canonical, node(ids::NETWORK)],
            vec![],
            Some(node("")),
        );
        let names: Vec<_> = tree
            .rows
            .iter()
            .filter_map(|row| {
                tree.node(&row.reference)
                    .map(|node| node.node.name.as_str())
            })
            .collect();
        assert_eq!(names, vec!["", ids::HOME, "c:\\known", ids::NETWORK]);
    }

    #[test]
    fn automatic_follow_opens_ancestors_and_manual_follow_can_open_the_current_folder() {
        let mut tree = NavTree::default();
        tree.set_roots(vec![node(ids::THIS_PC)], vec![], None);
        let root = tree.rows[0].reference.clone();
        let pc = tree.node_mut(&root).unwrap();
        pc.expanded = true;
        pc.children = Some(vec![TreeNode {
            node: node("C:\\"),
            expanded: true,
            children: Some(vec![TreeNode::new(node("C:\\Current"))]),
        }]);
        tree.flatten();
        let chain = vec![ids::THIS_PC.into(), "C:\\".into(), "C:\\Current".into()];
        let automatic = tree.follow(&chain, Follow::Ancestors).unwrap();
        assert_eq!(automatic.row, 2);
        assert!(!automatic.open);
        assert!(!automatic.loaded);
        let manual = tree.follow(&chain, Follow::Current).unwrap();
        assert_eq!(manual.row, 2);
        assert!(manual.open);
    }

    #[test]
    fn splitter_limits_leave_the_native_minimum_file_area() {
        assert_eq!(pane_width(-10.0, 798.0), 54.0);
        assert_eq!(pane_width(1000.0, 798.0), 660.0);
        assert_eq!(pane_width(1000.0, 884.0), 746.0);
        assert_eq!(pane_width(1000.0, 684.0), 546.0);
        assert_eq!(pane_width(196.0, 798.0), 196.0);
    }

    #[test]
    fn keyboard_pages_count_separator_rows_without_selecting_them() {
        let mut tree = NavTree::default();
        for (section, count) in [3, 8, 25].into_iter().enumerate() {
            tree.sections.push(
                (0..count)
                    .map(|index| TreeNode::new(node(&format!("{section}-{index}"))))
                    .collect(),
            );
        }
        tree.flatten();
        assert_eq!(tree.rows.len(), 38);
        assert_eq!(tree.page_target(6, 1, 22), 27);
        assert_eq!(tree.page_target(27, -1, 22), 6);
        assert_eq!(tree.page_target(0, 1, 22), 21);
        assert_eq!(tree.page_target(0, 1, 4), 4);
        assert_eq!(tree.page_target(7, -1, 5), 2);
    }
    #[test]
    fn changing_desktop_layout_preserves_ancestors_and_follows_fresh_library_visibility() {
        let pc = node(ids::THIS_PC);
        let library = node("::{031E4825-7B94-4DC3-B131-E946B44C8DD5}");
        let roots = vec![pc.clone(), library.clone(), node(ids::NETWORK)];
        let mut tree = NavTree::default();
        tree.set_roots(roots.clone(), vec![], None);
        tree.sections[2][0].expanded = true;
        tree.sections[2][0].children = Some(vec![TreeNode::new(node("C:\\"))]);
        tree.flatten();
        tree.set_roots(roots, vec![], Some(node("")));
        let drive = tree
            .rows
            .iter()
            .find(|r| tree.node(&r.reference).is_some_and(|n| n.node.id == "C:\\"))
            .unwrap();
        assert_eq!(drive.depth, 2);
        tree.sections[0][0].expanded = false;
        tree.set_roots(vec![pc.clone(), node(ids::NETWORK)], vec![], Some(node("")));
        assert_eq!(
            tree.rows.len(),
            1,
            "a refresh must not reopen a manually collapsed desktop"
        );
        assert!(!tree.sections[0][0]
            .children
            .as_ref()
            .unwrap()
            .iter()
            .any(|n| n.node.id == library.id));
        tree.set_roots(vec![pc, node(ids::NETWORK)], vec![], None);
        assert!(tree
            .rows
            .iter()
            .any(|r| r.depth == 1 && tree.node(&r.reference).is_some_and(|n| n.node.id == "C:\\")));
    }
}

pub fn tree_metrics(compact: bool) -> (f32, f32) {
    if compact {
        (24.0, 4.0)
    } else {
        (32.0, 6.0)
    }
}

pub fn pane_width(requested: f32, space: f32) -> f32 {
    requested.clamp(54.0, (space - 138.0).max(54.0)).round()
}

pub fn page_count(height: f32, row_height: f32) -> usize {
    ((height / row_height).floor() as usize)
        .saturating_sub(1)
        .max(1)
}

#[derive(Clone)]
pub struct Pick {
    pub reference: RowRef,
    pub id: String,
    pub at: String,
}

pub fn reveal_top(from: usize, to: usize, top: usize, fit: usize) -> Option<usize> {
    if from < top {
        Some(from)
    } else if to + 1 > top + fit {
        Some(if to - from + 1 <= fit {
            to + 1 - fit
        } else {
            from
        })
    } else {
        None
    }
}

pub const TREE_PIN: [u8; 14 * 13] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 31, 222, 177, 14, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 158, 255, 255, 208, 14, 0, 0, 0, 0, 0, 0, 0, 0, 31, 255, 255, 255, 255, 208, 14, 0,
    0, 0, 0, 0, 14, 130, 239, 255, 255, 255, 255, 255, 191, 0, 0, 0, 0, 111, 255, 255, 255, 255,
    255, 255, 255, 255, 208, 0, 0, 0, 0, 33, 224, 255, 255, 255, 255, 255, 241, 125, 16, 0, 0, 0,
    0, 0, 33, 241, 255, 255, 255, 158, 16, 0, 0, 0, 0, 0, 0, 0, 128, 255, 241, 255, 255, 64, 0, 0,
    0, 0, 0, 0, 0, 128, 255, 128, 33, 224, 255, 0, 0, 0, 0, 0, 0, 0, 64, 255, 128, 0, 0, 33, 97, 0,
    0, 0, 0, 0, 0, 0, 14, 64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0,
];

#[cfg(test)]
mod expansion_tests {
    use super::reveal_top;
    #[test]
    fn native_expansion_reveals_short_children_without_forcing_parent_to_top() {
        assert_eq!(reveal_top(30, 34, 0, 22), Some(13));
        assert_eq!(reveal_top(30, 80, 0, 22), Some(30));
        assert_eq!(reveal_top(30, 34, 20, 22), None);
        assert_eq!(reveal_top(5, 8, 20, 22), Some(5));
        assert_eq!(reveal_top(30, 30, 0, 22), Some(9));
    }
}
