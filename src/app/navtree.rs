use crate::app::ids;
use crate::shell::nav::NavNode;
use windows::core::GUID;

const CLSID_HOME: GUID = GUID::from_u128(0xf874310e_b6b7_47dc_bc84_b9e6b38f5903);
const CLSID_GALLERY: GUID = GUID::from_u128(0xe88865ea_0e1c_4e20_9aa6_edcd0212c87c);
const CLSID_ONEDRIVE: GUID = GUID::from_u128(0x018d5c66_4533_4307_9b53_224de2ed1fe6);
const CLSID_THIS_PC: GUID = GUID::from_u128(0x20d04fe0_3aea_1069_a2d8_08002b30309d);
const CLSID_LIBRARIES: GUID = GUID::from_u128(0x031e4825_7b94_4dc3_b131_e946b44c8dd5);
const CLSID_NETWORK: GUID = GUID::from_u128(0xf02c1a0d_be21_4350_88b0_7367fc96ef3c);
const PIN_GROUP: u32 = 0x45;

fn group(node: &NavNode) -> u32 {
    node.group_id.unwrap_or_else(|| {
        node.namespace_id()
            .and_then(|id| {
                [
                    (CLSID_HOME, 0x40),
                    (CLSID_GALLERY, 0x41),
                    (CLSID_ONEDRIVE, 0x42),
                    (CLSID_THIS_PC, 0x50),
                    (CLSID_LIBRARIES, 0x54),
                    (CLSID_NETWORK, 0x58),
                ]
                .into_iter()
                .find_map(|(clsid, group)| (id == clsid).then_some(group))
            })
            .unwrap_or_else(|| {
                if ids::is_drive_root(&node.id) {
                    0x56
                } else if node.is_fs {
                    0x47
                } else {
                    0x80
                }
            })
    })
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
            let mut roots = roots;
            roots.sort_by_key(group);
            let (mut home, devices): (Vec<_>, Vec<_>) =
                roots.into_iter().partition(|node| group(node) < PIN_GROUP);
            for node in &mut home {
                if node.namespace_id() == Some(CLSID_HOME) {
                    node.has_children = false;
                }
            }
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
            group_id: None,
            namespace_clsid: None,
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
    fn missing_groups_use_namespace_identity_without_guessing_from_an_empty_path() {
        let mut dropbox = node("C:\\Users\\A\\Dropbox");
        dropbox.is_fs = true;
        let mut onedrive = node("C:\\Users\\A\\OneDrive");
        onedrive.is_fs = true;
        onedrive.namespace_clsid = Some(CLSID_ONEDRIVE);
        let mut unknown = node("");
        unknown.is_fs = true;
        let mut tree = NavTree::default();
        tree.set_roots(
            vec![
                node(ids::THIS_PC),
                dropbox.clone(),
                unknown,
                onedrive.clone(),
                node(ids::HOME),
            ],
            vec![],
            None,
        );
        assert_eq!(
            tree.sections[0]
                .iter()
                .map(|n| n.node.id.as_str())
                .collect::<Vec<_>>(),
            vec![ids::HOME, onedrive.id.as_str()]
        );
        assert_eq!(
            tree.sections[2]
                .iter()
                .map(|n| n.node.id.as_str())
                .collect::<Vec<_>>(),
            vec![dropbox.id.as_str(), "", ids::THIS_PC]
        );
    }

    fn grouped(id: &str, group_id: u32) -> NavNode {
        let mut node = node(id);
        node.group_id = Some(group_id);
        node
    }

    fn section_ids(tree: &NavTree, section: usize) -> Vec<&str> {
        tree.sections[section]
            .iter()
            .map(|node| node.node.id.as_str())
            .collect()
    }

    #[test]
    fn native_groups_keep_shell_ties_and_quick_access_order_with_a_physical_onedrive_path() {
        let mut onedrive = grouped("C:\\Users\\A\\OneDrive", 66);
        onedrive.namespace_clsid = Some(CLSID_ONEDRIVE);
        onedrive.is_fs = true;
        let mut first_pin = grouped("C:\\Z pinned", 69);
        first_pin.is_pinned = true;
        let mut second_pin = grouped("C:\\A pinned", 69);
        second_pin.is_pinned = true;
        let mut tree = NavTree::default();
        tree.set_roots(
            vec![
                grouped(ids::THIS_PC, 80),
                grouped("C:\\Z cloud", 71),
                grouped(ids::HOME, 64),
                onedrive,
                grouped("C:\\A cloud", 71),
                grouped(ids::GALLERY, 65),
            ],
            vec![first_pin, grouped("C:\\Frequent", 69), second_pin],
            None,
        );
        assert_eq!(
            section_ids(&tree, 0),
            vec![ids::HOME, ids::GALLERY, "C:\\Users\\A\\OneDrive"]
        );
        assert_eq!(
            section_ids(&tree, PINS),
            vec!["C:\\Z pinned", "C:\\A pinned"]
        );
        assert_eq!(
            section_ids(&tree, 2),
            vec!["C:\\Z cloud", "C:\\A cloud", ids::THIS_PC]
        );
        assert!(!tree.sections[0][0].node.has_children);
        assert!(tree.sections[0][1].node.has_children);
    }

    #[test]
    fn reported_group_overrides_namespace_fallback_and_zero_is_a_valid_group() {
        let mut onedrive = grouped("C:\\Users\\A\\OneDrive", 71);
        onedrive.namespace_clsid = Some(CLSID_ONEDRIVE);
        let mut home = node("C:\\Home parsing alias");
        home.namespace_clsid = Some(CLSID_HOME);
        let mut tree = NavTree::default();
        tree.set_roots(
            vec![onedrive, home, grouped("C:\\Explicit zero", 0)],
            vec![],
            None,
        );
        assert_eq!(
            section_ids(&tree, 0),
            vec!["C:\\Explicit zero", "C:\\Home parsing alias"]
        );
        assert!(tree.sections[0][0].node.has_children);
        assert!(!tree.sections[0][1].node.has_children);
        assert_eq!(section_ids(&tree, 2), vec!["C:\\Users\\A\\OneDrive"]);
    }

    #[test]
    fn a_late_root_is_regrouped_on_refresh_without_losing_expanded_children() {
        let home = grouped(ids::HOME, 64);
        let gallery = grouped(ids::GALLERY, 65);
        let pc = grouped(ids::THIS_PC, 80);
        let mut pin = grouped("C:\\Pinned", 69);
        pin.is_pinned = true;
        let mut tree = NavTree::default();
        tree.set_roots(
            vec![home.clone(), gallery.clone(), pc.clone()],
            vec![pin.clone()],
            None,
        );
        tree.sections[2][0].expanded = true;
        tree.sections[2][0].children = Some(vec![TreeNode::new(node("C:\\"))]);
        tree.flatten();
        let mut late = grouped("C:\\Users\\A\\OneDrive", 66);
        late.namespace_clsid = Some(CLSID_ONEDRIVE);
        tree.set_roots(vec![home, gallery, pc, late], vec![pin], None);
        assert_eq!(
            section_ids(&tree, 0),
            vec![ids::HOME, ids::GALLERY, "C:\\Users\\A\\OneDrive"]
        );
        assert_eq!(section_ids(&tree, PINS), vec!["C:\\Pinned"]);
        assert!(tree.sections[2][0].expanded);
        assert_eq!(
            tree.sections[2][0].children.as_ref().unwrap()[0].node.id,
            "C:\\"
        );
        assert!(tree.rows.iter().any(|row| row.depth == 1
            && tree
                .node(&row.reference)
                .is_some_and(|node| node.node.id == "C:\\")));
    }

    #[test]
    fn missing_group_does_not_treat_guid_text_inside_a_physical_path_as_a_namespace() {
        let mut unknown = node("C:\\{018D5C66-4533-4307-9B53-224DE2ED1FE6}\\files");
        unknown.is_fs = true;
        let mut tree = NavTree::default();
        tree.set_roots(vec![unknown, node(ids::HOME)], vec![], None);
        assert_eq!(section_ids(&tree, 0), vec![ids::HOME]);
        assert_eq!(tree.sections[2].len(), 1);
    }

    #[test]
    fn drive_roots_follow_this_pc_before_network() {
        let mut drive = node("E:\\");
        drive.is_fs = true;
        let order = [node(ids::THIS_PC), drive, node(ids::NETWORK)].map(|n| group(&n));
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]));
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
