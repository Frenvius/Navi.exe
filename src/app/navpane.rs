//! The navigation pane's tree behavior: its own selection (the keyboard moves it without
//! navigating, Enter goes there), expanding and collapsing, and item-aligned scrolling.

use std::rc::Rc;
use std::time::Instant;

use slint::platform::Key;
use slint::ComponentHandle;

use super::navtree::RowRef;
use super::{ids, is, App, TYPEAHEAD_RESET_MS};
use crate::Shell;

/// Tree rows are 24px; the first one starts 4px down.
const ROW: f32 = 24.0;
const TOP: f32 = 4.0;

/// A row the user picked in the tree, remembered by its place and id, with the location
/// shown when it was picked: it stays selected until the view goes somewhere else.
#[derive(Clone)]
pub struct NavPick {
    pub reference: RowRef,
    pub id: String,
    pub at: String,
}

impl App {
    /// The row the tree shows selected: the picked one while it holds, else the deepest
    /// visible ancestor of the location.
    pub fn nav_selected(&self) -> Option<usize> {
        let st = self.state.borrow();
        let picked = st.nav_pick.as_ref().filter(|p| ids::same(&p.at, &st.path) || ids::same(&p.id, &st.path)).and_then(|p| {
            st.nav.rows.iter().position(|r| {
                r.reference == p.reference && st.nav.node(&r.reference).map(|n| ids::same(&n.node.id, &p.id)).unwrap_or(false)
            })
        });
        picked.or_else(|| {
            let mut chain: Vec<String> = st.crumbs.iter().map(|c| c.id.clone()).collect();
            if let Some(root) = st.crumbs.first().and_then(|c| c.nav_root.as_ref()) {
                // Rebasing the address under This PC must not rebase its tree
                // selection. A hidden profile ancestor falls back to Home.
                if let Some(first) = chain.first_mut() {*first = root.clone();}
                chain.insert(0, ids::HOME.into());
            }
            st.nav.selected_row(&chain)
        })
    }

    pub fn nav_pick(&self, row: usize) {
        let mut st = self.state.borrow_mut();
        let Some(reference) = st.nav.rows.get(row).map(|r| r.reference.clone()) else { return };
        let Some(id) = st.nav.node(&reference).map(|n| n.node.id.clone()) else { return };
        let at = st.path.clone();
        st.nav_pick = Some(NavPick { reference, id, at });
    }

    /// A click on a row: it is selected and shown, the tree keeping the focus.
    pub fn nav_clicked(self: &Rc<Self>, row: usize) {
        let Some((node, _)) = self.nav_node(row) else { return };
        self.nav_pick(row);
        self.navigate(&node.id);
        self.sync_nav();
        self.ui().invoke_focus_tree();
    }

    fn nav_is_node(&self, row: usize) -> bool {
        self.state.borrow().nav.rows.get(row).map(|r| r.reference != RowRef::Separator).unwrap_or(false)
    }

    /// The nearest node row from `row` going `step` (skipping separators), or None.
    fn nav_step(&self, row: usize, step: isize) -> Option<usize> {
        let n = self.state.borrow().nav.rows.len() as isize;
        let mut at = row as isize + step;
        while at >= 0 && at < n {
            if self.nav_is_node(at as usize) {
                return Some(at as usize);
            }
            at += step.signum();
        }
        None
    }

    fn nav_parent(&self, row: usize) -> Option<usize> {
        let st = self.state.borrow();
        let depth = st.nav.rows.get(row)?.depth;
        (0..row).rev().find(|&r| st.nav.rows[r].reference != RowRef::Separator && st.nav.rows[r].depth < depth)
    }

    /// Moves the tree's selection (not the view) to a row and scrolls it into view.
    fn nav_move(&self, row: usize) {
        self.nav_pick(row);
        self.sync_nav();
        self.nav_reveal(row, row, false);
    }

    pub fn nav_key(self: &Rc<Self>, text: &str, ctrl: bool) -> bool {
        let n = self.state.borrow().nav.rows.len();
        if n == 0 {
            return false;
        }
        let current = self.nav_selected();
        let first = (0..n).find(|&r| self.nav_is_node(r));
        let Some(cur) = current.or(first) else { return false };
        let page = self.nav_extent().0 as isize;
        let (expandable, expanded, depth) = {
            let st = self.state.borrow();
            let row = &st.nav.rows[cur];
            let node = st.nav.node(&row.reference);
            (node.map(|n| n.node.has_children).unwrap_or(false), node.map(|n| n.expanded).unwrap_or(false), row.depth)
        };
        if is(text, Key::DownArrow) {
            let to = if current.is_none() { Some(cur) } else { self.nav_step(cur, 1) };
            if let Some(to) = to {
                self.nav_move(to);
            }
        } else if is(text, Key::UpArrow) {
            if let Some(to) = self.nav_step(cur, -1) {
                self.nav_move(to);
            }
        } else if is(text, Key::Home) {
            if let Some(to) = first {
                self.nav_move(to);
            }
        } else if is(text, Key::End) {
            if let Some(to) = self.nav_step(n, -1) {
                self.nav_move(to);
            }
        } else if is(text, Key::PageDown) {
            let mut to = cur;
            for _ in 0..page {
                match self.nav_step(to, 1) {
                    Some(r) => to = r,
                    None => break,
                }
            }
            self.nav_move(to);
        } else if is(text, Key::PageUp) {
            let mut to = cur;
            for _ in 0..page {
                match self.nav_step(to, -1) {
                    Some(r) => to = r,
                    None => break,
                }
            }
            self.nav_move(to);
        } else if is(text, Key::RightArrow) {
            if expandable && !expanded {
                self.nav_pick(cur);
                self.toggle_nav(cur);
            } else if expanded {
                let child = self.nav_step(cur, 1).filter(|&r| self.state.borrow().nav.rows[r].depth > depth);
                if let Some(child) = child {
                    self.nav_move(child);
                }
            }
        } else if is(text, Key::LeftArrow) {
            if expanded {
                self.nav_pick(cur);
                self.toggle_nav(cur);
            } else if let Some(parent) = self.nav_parent(cur) {
                self.nav_move(parent);
            }
        } else if is(text, Key::Backspace) {
            if let Some(parent) = self.nav_parent(cur) {
                self.nav_move(parent);
            }
        } else if is(text, Key::Return) {
            self.nav_clicked(cur);
        } else if text.chars().count() == 1 && !ctrl && !text.chars().next().map(char::is_control).unwrap_or(true) {
            self.nav_typeahead(cur, text);
        } else {
            return false;
        }
        true
    }

    /// Type-ahead: the next row whose name starts with what was typed in the last second.
    fn nav_typeahead(&self, cur: usize, text: &str) {
        let prefix = {
            let mut st = self.state.borrow_mut();
            let (at, typed) = &mut st.nav_typeahead;
            if at.elapsed().as_millis() > TYPEAHEAD_RESET_MS {
                typed.clear();
            }
            *at = Instant::now();
            typed.push_str(&text.to_lowercase());
            typed.clone()
        };
        // A repeated letter cycles through the rows starting with it.
        let single = prefix.chars().all(|c| Some(c) == prefix.chars().next());
        let (start, needle) = if single { (cur + 1, prefix.chars().take(1).collect::<String>()) } else { (cur, prefix) };
        let found = {
            let st = self.state.borrow();
            let n = st.nav.rows.len();
            (0..n).map(|i| (start + i) % n).find(|&r| {
                st.nav.node(&st.nav.rows[r].reference).map(|node| node.node.name.to_lowercase().starts_with(&needle)).unwrap_or(false)
            })
        };
        if let Some(row) = found {
            self.nav_move(row);
        }
    }

    /// Scrolls the tree, a row at a time, so rows `from`..=`to` show: when `to` is below the
    /// pane, `from` goes to the top (as far as the rows scroll), as the tree view does when
    /// it expands an item; above it, `from` comes to the top.
    pub fn nav_reveal(&self, from: usize, to: usize, growing: bool) {
        if let Some(top)=reveal_top(from,to,self.nav_top(),self.nav_extent().0) {
            self.nav_scroll_to(top,growing);
        }
    }

    /// The rows that fit in the pane, and the last row that can scroll to the top. The tree
    /// counts a row fewer than its height holds (TVM_GETVISIBLECOUNT is 22 at 559px), which
    /// sets its page, its scroll range and when its bar shows.
    pub(super) fn nav_extent(&self) -> (usize, usize) {
        let page = self.ui().global::<Shell>().get_nav_page();
        let n = self.state.borrow().nav.rows.len();
        let fit = ((page / ROW).floor() as usize).saturating_sub(1).max(1);
        (fit, n.saturating_sub(fit))
    }

    /// The row at the top of the pane.
    fn nav_top(&self) -> usize {
        let scroll = -self.ui().global::<Shell>().get_nav_scroll_y();
        if scroll <= 0.0 {
            0
        } else {
            ((scroll - TOP) / ROW).round().max(0.0) as usize
        }
    }

    /// `growing`: rows were just added, which the list only counts once it has laid them
    /// out (it clamps the offset to its old height meanwhile); the pane applies it again then.
    fn nav_scroll_to(&self, top: usize, growing: bool) {
        let top = top.min(self.nav_extent().1);
        let y = if top == 0 { 0.0 } else { TOP + ROW * top as f32 };
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        shell.set_nav_scroll_y(-y);
        if growing {
            shell.set_nav_scroll_want(y);
        }
    }

    /// Fewer rows (collapsed, refreshed): the tree scrolls back so none is left blank below.
    pub fn nav_clamp(&self) {
        let last = self.nav_extent().1;
        if self.nav_top() > last {
            self.nav_scroll_to(last, false);
        }
    }

    /// The tree row at a point of the pane (`pane` wide), if it is an item.
    pub fn nav_row_at(&self, x: f32, y: f32, pane: f32) -> Option<usize> {
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let n = self.state.borrow().nav.rows.len();
        let scrolls = self.nav_extent().1 > 0;
        if x < 0.0 || x >= pane - if scrolls { 17.0 } else { 0.0 } {
            return None;
        }
        let content = y - shell.get_nav_scroll_y() - TOP;
        if content < 0.0 {
            return None;
        }
        let row = (content / ROW) as usize;
        (row < n && self.nav_is_node(row)).then_some(row)
    }

    /// Scrolls the tree a row up (-1) or down (1).
    pub fn nav_scroll_by(&self, rows: i32) {
        let top = self.nav_top() as i64 + rows as i64;
        self.nav_scroll_to(top.max(0) as usize, false);
    }

    /// The bar's track: a page is a row less than the rows that fit.
    pub fn nav_pages(&self, pages: i32) {
        let step = self.nav_extent().0.saturating_sub(1).max(1) as i32;
        self.nav_scroll_by(pages * step);
    }

    /// The bar's thumb was dragged to a pixel offset: the tree scrolls whole rows.
    pub fn nav_snap(&self) {
        self.nav_scroll_to(self.nav_top(), false);
    }

    /// The wheel over the pane: three rows a notch, a row at the top.
    pub fn nav_wheel(&self, steps: i32) {
        let top = self.nav_top() as i64 - 3 * steps as i64;
        self.nav_scroll_to(top.max(0) as usize, false);
    }

    /// Ctrl+Shift+E: the tree opens down to the location, which it then shows selected. Each
    /// level whose children aren't loaded yet continues when they arrive (`nav_expanding`).
    pub fn nav_expand_to_current(self: &Rc<Self>) {
        self.state.borrow_mut().nav_expand_current=true;
        self.nav_continue_expansion();
    }
    /// Folder Options follows ancestors, keeping the current folder closed. Revealing
    /// the selection uses the tree's normal page, unlike Ctrl+Shift+E's top alignment.
    pub fn nav_follow_current(self: &Rc<Self>) {
        self.state.borrow_mut().nav_expand_current=false;
        self.nav_continue_expansion();
    }
    pub fn nav_continue_expansion(self: &Rc<Self>) {
        {
            let mut st=self.state.borrow_mut();
            if !st.nav_expand_current && !st.nav_options.expand {st.nav_expanding=false;return}
        }
        self.state.borrow_mut().nav_pick = None;
        loop {
            let (row, open, loaded) = {
                let st = self.state.borrow();
                let chain: Vec<String> = st.crumbs.iter().map(|c| c.id.clone()).collect();
                let Some(row) = st.nav.selected_row(&chain).or_else(||st.nav_options.all.then_some(0)) else { break };
                let Some(node) = st.nav.node(&st.nav.rows[row].reference) else { break };
                // The folder itself opens too (C:\Windows shows its subfolders).
                let current=chain.last().is_some_and(|id|ids::same(id,&node.node.id));
                let open = !node.expanded && node.node.has_children && (st.nav_expand_current || !current);
                (row, open, node.children.is_some())
            };
            if !open {
                self.state.borrow_mut().nav_expanding = false;
                self.sync_nav();
                let manual=self.state.borrow().nav_expand_current;
                if manual {
                    let reference=self.state.borrow().nav.rows[row].reference.clone();
                    self.nav_reveal_children(&reference);
                } else {self.nav_reveal(row,row,true);}
                // ListView can retain a whole-row offset while new rows settle,
                // dropping the 4px leading margin. Normalize after layout without
                // undoing an intervening user scroll or navigation.
                let top=self.nav_top();
                let path=self.state.borrow().path.clone();
                let weak=Rc::downgrade(self);
                slint::Timer::single_shot(std::time::Duration::from_millis(32),move||{
                    if let Some(app)=weak.upgrade(){
                        let valid={let st=app.state.borrow();st.path==path && st.nav_expand_current==manual && (manual || st.nav_options.expand)};
                        if valid && app.nav_top()==top && app.nav_selected()==Some(row){app.nav_scroll_to(top,false);}
                    }
                });
                return;
            }
            self.state.borrow_mut().nav_expanding = !loaded;
            self.expand_nav_quietly(row);
            if !loaded {
                return;
            }
        }
        self.state.borrow_mut().nav_expanding = false;
    }

    /// After expanding a row: its children scroll into view, the row itself staying shown.
    pub fn nav_reveal_children(&self, reference: &RowRef) {
        let (row, last) = {
            let st = self.state.borrow();
            let Some(row) = st.nav.rows.iter().position(|r| r.reference == *reference) else { return };
            let depth = st.nav.rows[row].depth;
            let mut last = row;
            while last + 1 < st.nav.rows.len() && st.nav.rows[last + 1].reference != RowRef::Separator && st.nav.rows[last + 1].depth > depth {
                last += 1;
            }
            (row, last)
        };
        self.nav_reveal(row, last, true);
    }
}

/// Reveal a short expanded subtree at the bottom; a subtree taller than the page
/// starts with its parent at the top. Single-row keyboard/automatic reveals use
/// that same last-visible-row rule.
fn reveal_top(from:usize,to:usize,top:usize,fit:usize)->Option<usize>{
    if from<top {Some(from)}
    else if to+1>top+fit {Some(if to-from+1<=fit {to+1-fit}else{from})}
    else {None}
}
#[cfg(test)]
mod tests {
    use super::reveal_top;
    #[test]
    fn native_expansion_reveals_short_children_without_forcing_parent_to_top(){
        assert_eq!(reveal_top(30,34,0,22),Some(13));
        assert_eq!(reveal_top(30,80,0,22),Some(30));
        assert_eq!(reveal_top(30,34,20,22),None);
        assert_eq!(reveal_top(5,8,20,22),Some(5));
        assert_eq!(reveal_top(30,30,0,22),Some(9));
    }
}
