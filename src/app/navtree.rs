//! Navigation pane tree: Explorer's three groups (Home cluster, Quick access pins, devices),
//! flattened into rows for the UI.

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
        TreeNode { node, expanded: false, children: None }
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
        (RowRef::Node { section: a, path: p }, RowRef::Node { section: b, path: q }) => a == b && p.len() > q.len() && p.starts_with(q),
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

impl NavTree {
    pub fn set_roots(&mut self, roots: Vec<NavNode>, pins: Vec<NavNode>, desktop: Option<NavNode>) {
        let mut home: Vec<NavNode> = roots.iter().filter(|n| rank(n) <= 2).cloned().collect();
        let mut devices: Vec<NavNode> = roots.iter().filter(|n| rank(n) > 2).cloned().collect();
        home.sort_by_key(rank);
        devices.sort_by_key(rank);
        for node in home.iter_mut() {
            // Home lists recent content, not folders, so it never expands.
            if rank(node) == 0 {
                node.has_children = false;
            }
        }
        let pins: Vec<NavNode> = pins.into_iter().filter(|p| p.is_pinned).collect();

        let old = std::mem::take(&mut self.sections);
        self.sections = if let Some(desktop) = desktop {
            vec![vec![TreeNode{node:desktop,expanded:true,children:Some(roots.into_iter().map(TreeNode::new).collect())}],Vec::new(),Vec::new()]
        } else { vec![
            home.into_iter().map(TreeNode::new).collect(),
            pins.into_iter().map(TreeNode::new).collect(),
            devices.into_iter().map(TreeNode::new).collect(),
        ] };
        // Preserve expanded nodes by identity when the desktop layout or library row
        // changes; merge fresh desktop children so hidden roots don't reappear.
        fn collect(nodes:&[TreeNode], kept:&mut std::collections::HashMap<String,TreeNode>) {
            for node in nodes {
                if node.expanded || node.node.id.is_empty() { kept.insert(node.node.id.to_ascii_lowercase(),node.clone()); }
                if let Some(children)=&node.children {collect(children,kept)}
            }
        }
        fn restore(nodes:&mut [TreeNode],kept:&std::collections::HashMap<String,TreeNode>) {
            for node in nodes {
                if let Some(old)=kept.get(&node.node.id.to_ascii_lowercase()) {
                    node.expanded=old.expanded;
                    if node.children.is_none(){node.children=old.children.clone();}
                }
                if let Some(children)=&mut node.children {restore(children,kept)}
            }
        }
        let mut kept=std::collections::HashMap::new();
        for section in &old {collect(section,&mut kept)}
        for section in &mut self.sections {restore(section,&kept)}
        self.flatten();
    }

    pub fn node(&self, reference: &RowRef) -> Option<&TreeNode> {
        let RowRef::Node { section, path } = reference else { return None };
        let (last, parents) = path.split_last()?;
        let mut list = self.sections.get(*section)?;
        for &i in parents {
            list = list.get(i)?.children.as_ref()?;
        }
        list.get(*last)
    }

    pub fn node_mut(&mut self, reference: &RowRef) -> Option<&mut TreeNode> {
        let RowRef::Node { section, path } = reference else { return None };
        let (last, parents) = path.split_last()?;
        let mut list = self.sections.get_mut(*section)?;
        for &i in parents {
            list = list.get_mut(i)?.children.as_mut()?;
        }
        list.get_mut(*last)
    }

    pub fn flatten(&mut self) {
        fn walk(nodes: &[TreeNode], section: usize, prefix: &mut Vec<usize>, depth: i32, rows: &mut Vec<Row>) {
            for (i, node) in nodes.iter().enumerate() {
                prefix.push(i);
                rows.push(Row { depth, reference: RowRef::Node { section, path: prefix.clone() } });
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
                rows.push(Row { depth: 0, reference: RowRef::Separator });
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
            let RowRef::Node { section, .. } = &row.reference else { continue };
            let Some(node) = self.node(&row.reference) else { continue };
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
                self.node(&row.reference).map(|n| ids::same(&n.node.id, current)).unwrap_or(false)
            });
        }
        best.map(|(_, row)| row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(id:&str)->NavNode {NavNode{name:id.into(),id:id.into(),icon:0,has_children:true,is_fs:false,is_pinned:false}}
    #[test]
    fn changing_desktop_layout_preserves_ancestors_and_follows_fresh_library_visibility(){
        let pc=node(ids::THIS_PC);let library=node("::{031E4825-7B94-4DC3-B131-E946B44C8DD5}");
        let roots=vec![pc.clone(),library.clone(),node(ids::NETWORK)];
        let mut tree=NavTree::default();tree.set_roots(roots.clone(),vec![],None);
        tree.sections[2][0].expanded=true;
        tree.sections[2][0].children=Some(vec![TreeNode::new(node("C:\\"))]);
        tree.flatten();
        tree.set_roots(roots,vec![],Some(node("")));
        let drive=tree.rows.iter().find(|r|tree.node(&r.reference).is_some_and(|n|n.node.id=="C:\\")).unwrap();
        assert_eq!(drive.depth,2);
        tree.sections[0][0].expanded=false;
        tree.set_roots(vec![pc.clone(),node(ids::NETWORK)],vec![],Some(node("")));
        assert_eq!(tree.rows.len(),1,"a refresh must not reopen a manually collapsed desktop");
        assert!(!tree.sections[0][0].children.as_ref().unwrap().iter().any(|n|n.node.id==library.id));
        tree.set_roots(vec![pc,node(ids::NETWORK)],vec![],None);
        assert!(tree.rows.iter().any(|r|r.depth==1 && tree.node(&r.reference).is_some_and(|n|n.node.id=="C:\\")));
    }
}
