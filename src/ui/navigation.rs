use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc};
use windows::core::Result;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::app::navtree::{NavTree, Pick};
use crate::shell::{icons::Rgba, nav::NavNode, nav_options::Options};

pub const READY: u32 = WM_APP + 0x62;
pub const CHANGED: u32 = WM_APP + 0x63;
#[cfg(debug_assertions)]
pub const DIAGNOSTICS: u32 = WM_APP + 0x64;

pub struct Notifications {
    registration: u32,
    _roots: Vec<crate::shell::Pidl>,
    paths: Vec<String>,
}

impl Notifications {
    pub fn new(hwnd: HWND) -> Result<Self> {
        Self::watch(
            hwnd,
            vec![
                String::new(),
                crate::app::ids::QUICK_ACCESS.into(),
                crate::app::ids::THIS_PC.into(),
            ],
        )
    }

    fn watch(hwnd: HWND, paths: Vec<String>) -> Result<Self> {
        Self::watch_message(hwnd, paths, CHANGED)
    }

    pub fn location(hwnd: HWND, path: &str) -> Result<Self> {
        Self::watch_message(hwnd, vec![path.to_owned()], super::location::CHANGED)
    }

    fn watch_message(hwnd: HWND, paths: Vec<String>, message: u32) -> Result<Self> {
        use windows::Win32::UI::Shell::*;
        let roots = paths
            .iter()
            .map(|path| crate::shell::Pidl::parse(path))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| {
                windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
            })?;
        let entries: Vec<_> = roots
            .iter()
            .map(|root| SHChangeNotifyEntry {
                pidl: root.ptr() as *mut _,
                fRecursive: false.into(),
            })
            .collect();
        let events = SHCNE_MKDIR
            | SHCNE_RMDIR
            | SHCNE_RENAMEFOLDER
            | SHCNE_UPDATEDIR
            | SHCNE_DRIVEADD
            | SHCNE_DRIVEADDGUI
            | SHCNE_DRIVEREMOVED
            | SHCNE_NETSHARE
            | SHCNE_NETUNSHARE
            | SHCNE_UPDATEITEM;
        let registration = unsafe {
            SHChangeNotifyRegister(
                hwnd,
                SHCNRF_ShellLevel | SHCNRF_InterruptLevel | SHCNRF_NewDelivery,
                if message == CHANGED {
                    events.0 as i32
                } else {
                    SHCNE_ALLEVENTS.0 as i32
                },
                message,
                entries.len() as i32,
                entries.as_ptr(),
            )
        };
        if registration == 0 {
            return Err(windows::core::Error::from_win32());
        }
        Ok(Self {
            registration,
            _roots: roots,
            paths,
        })
    }

    pub fn sync(&mut self, hwnd: HWND, parents: &[String]) -> Result<()> {
        let mut paths = vec![
            String::new(),
            crate::app::ids::QUICK_ACCESS.into(),
            crate::app::ids::THIS_PC.into(),
        ];
        paths.extend_from_slice(parents);
        paths.sort_by_key(|path| path.to_ascii_lowercase());
        paths.dedup_by(|left, right| crate::app::ids::same(left, right));
        if paths == self.paths {
            return Ok(());
        }
        *self = Self::watch(hwnd, paths)?;
        Ok(())
    }
}

impl Drop for Notifications {
    fn drop(&mut self) {
        if !unsafe { windows::Win32::UI::Shell::SHChangeNotifyDeregister(self.registration) }
            .as_bool()
        {
            eprintln!(
                "native navigation notifications release: {}",
                windows::core::Error::from_win32()
            );
        }
    }
}

#[derive(Clone, Copy)]
enum ChildLoad {
    Expansion,
    Refresh,
}

enum Request {
    Roots(usize),
    Children(String, ChildLoad),
    Icons(Vec<u32>),
}

enum Reply {
    Roots(
        usize,
        Options,
        Vec<NavNode>,
        Vec<NavNode>,
        Option<NavNode>,
        (f32, bool),
    ),
    Children(String, ChildLoad, Vec<NavNode>),
    Icons(Vec<(u32, Arc<Rgba>)>, Vec<u32>),
    Error(String),
}

pub struct Worker {
    sender: mpsc::Sender<super::address::Request>,
    replies: mpsc::Sender<Reply>,
    receiver: mpsc::Receiver<Reply>,
    owner: usize,
}

pub struct Work {
    request: Request,
    replies: mpsc::Sender<Reply>,
    owner: usize,
}

impl Work {
    pub fn run(self) {
        let result = (|| -> std::result::Result<Reply, String> {
            match self.request {
                Request::Roots(generation) => {
                    if super::memory::enabled() {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                    super::memory::checkpoint("namespace-before");
                    let options = crate::shell::nav_options::read();
                    let reply = Reply::Roots(
                        generation,
                        options,
                        crate::shell::nav::children("")?,
                        crate::shell::nav::children(crate::app::ids::QUICK_ACCESS)?,
                        options.all.then(crate::shell::nav::desktop).flatten(),
                        (
                            crate::shell::nav_options::pane_width(),
                            crate::shell::nav_options::pane_visible(),
                        ),
                    );
                    super::memory::checkpoint("namespace-roots");
                    Ok(reply)
                }
                Request::Children(id, reason) => Ok(Reply::Children(
                    id.clone(),
                    reason,
                    crate::shell::nav::children(&id)?,
                )),
                Request::Icons(indices) => {
                    let mut images = Vec::new();
                    let mut failures = Vec::new();
                    for index in indices {
                        let image = match super::icons::small(index) {
                            Ok(image) => image,
                            Err(error) => {
                                eprintln!("native tree icon {index}: {error}");
                                match super::icons::small(crate::shell::icons::folder_icon()) {
                                    Ok(image) => image,
                                    Err(error) => {
                                        eprintln!("native tree fallback icon: {error}");
                                        failures.push(index);
                                        continue;
                                    }
                                }
                            }
                        };
                        images.push((index, image));
                    }
                    super::memory::checkpoint("namespace-icons");
                    Ok(Reply::Icons(images, failures))
                }
            }
        })();
        if self
            .replies
            .send(result.unwrap_or_else(Reply::Error))
            .is_err()
        {
            return;
        }
        if let Err(error) =
            unsafe { PostMessageW(Some(HWND(self.owner as _)), READY, WPARAM(0), LPARAM(0)) }
        {
            eprintln!("native navigation notification: {error}");
        }
    }
}

impl Worker {
    pub fn start(hwnd: HWND, address: &super::address::Worker) -> Self {
        let (replies, receiver) = mpsc::channel();
        Self {
            sender: address.dispatcher(),
            replies,
            receiver,
            owner: hwnd.0 as usize,
        }
    }
    fn send(&self, request: Request) {
        let work = Work {
            request,
            replies: self.replies.clone(),
            owner: self.owner,
        };
        if let Err(error) = self.sender.send(super::address::Request::Navigation(work)) {
            eprintln!("native navigation request: {error}");
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Row(usize),
    Expando(usize),
    Bar(super::scroll::Part),
    Splitter,
    Background,
}

pub struct State {
    pub tree: NavTree,
    pub options: Options,
    pub width: f32,
    pub visible: bool,
    pub height: f32,
    pub compact: bool,
    pub top: usize,
    pub selected: Option<usize>,
    pub pick: Option<Pick>,
    pub hover: Option<Hit>,
    pub drop_row: Option<usize>,
    pub insert_y: Option<f32>,
    pub pressed: Option<Hit>,
    pub pointer_inside: bool,
    pub focused: bool,
    pub active: bool,
    pub revision: usize,
    pub dirty_rows: Option<(usize, usize)>,
    pub pointer: Option<(f32, f32)>,
    pub opacity: f32,
    pub fade: Option<(std::time::Instant, f32, f32, f32)>,
    pub split_drag: Option<(f32, f32)>,
    pub thumb_drag: Option<(f32, usize)>,
    pub images: HashMap<u32, Arc<Rgba>>,
    pub pinned_parents: Vec<String>,
    pub manual_follow: bool,
    pub follow_pending: bool,
    pub expanding_all: HashSet<String>,
    pub typeahead: (std::time::Instant, String),
    pending_images: HashSet<u32>,
    failed_images: HashSet<u32>,
    pending_children: HashSet<String>,
    root_generation: usize,
    followed: String,
    wheel: i32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            tree: Default::default(),
            options: Default::default(),
            width: crate::shell::nav_options::pane_width(),
            visible: crate::shell::nav_options::pane_visible(),
            height: 0.0,
            compact: true,
            top: 0,
            selected: None,
            pick: None,
            hover: None,
            drop_row: None,
            insert_y: None,
            pressed: None,
            pointer_inside: false,
            focused: false,
            active: true,
            revision: 1,
            dirty_rows: None,
            pointer: None,
            opacity: 0.0,
            fade: None,
            split_drag: None,
            thumb_drag: None,
            images: Default::default(),
            pinned_parents: Vec::new(),
            manual_follow: false,
            follow_pending: false,
            expanding_all: Default::default(),
            typeahead: (std::time::Instant::now(), String::new()),
            pending_images: Default::default(),
            failed_images: Default::default(),
            pending_children: Default::default(),
            root_generation: 0,
            followed: String::new(),
            wheel: 0,
        }
    }
}

impl State {
    pub fn left(&self) -> f32 {
        if self.visible {
            self.width + 6.0
        } else {
            0.0
        }
    }
    pub fn metrics(&self) -> (f32, f32) {
        crate::app::navtree::tree_metrics(self.compact)
    }
    pub fn page(&self) -> usize {
        crate::app::navtree::page_count(self.height, self.metrics().0)
    }
    pub fn maximum(&self) -> usize {
        self.tree.rows.len().saturating_sub(self.page())
    }
    pub fn offset(&self) -> f32 {
        if self.top == 0 {
            0.0
        } else {
            self.metrics().1 + self.metrics().0 * self.top as f32
        }
    }
    pub fn content_width(&self) -> f32 {
        self.width - if self.maximum() > 0 { 17.0 } else { 0.0 }
    }
    pub fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn dirty(&mut self, row: Option<usize>) {
        if let Some(row) = row {
            self.dirty_rows = Some(
                self.dirty_rows
                    .map_or((row, row + 1), |old| (old.0.min(row), old.1.max(row + 1))),
            );
        }
    }

    pub fn set_hover(&mut self, hit: Option<Hit>) {
        if hit == self.hover {
            return;
        }
        for target in [self.hover, hit] {
            self.dirty(match target {
                Some(Hit::Row(row) | Hit::Expando(row)) => Some(row),
                _ => None,
            });
        }
        self.hover = hit;
    }

    fn rehover(&mut self) {
        self.set_hover(self.pointer.and_then(|point| self.hit(point)));
    }

    pub fn refresh(&mut self, worker: &Worker) {
        self.root_generation += 1;
        worker.send(Request::Roots(self.root_generation));
    }

    pub fn reload(&mut self, worker: &Worker) {
        self.refresh(worker);
        for row in &self.tree.rows {
            if let Some(node) = self.tree.node(&row.reference).filter(|node| node.expanded) {
                if self.pending_children.insert(node.node.id.clone()) {
                    worker.send(Request::Children(node.node.id.clone(), ChildLoad::Refresh));
                }
            }
        }
    }

    pub fn receive(&mut self, worker: &Worker) {
        while let Ok(reply) = worker.receiver.try_recv() {
            match reply {
                Reply::Roots(generation, options, roots, pins, desktop, pane)
                    if generation == self.root_generation =>
                {
                    if options.all != self.options.all {
                        self.top = 0;
                    }
                    if options.expand
                        && (!self.options.expand
                            || options.all != self.options.all
                            || self.tree.rows.is_empty())
                    {
                        self.follow_pending = true;
                    }
                    if self.split_drag.is_none() {
                        self.width = pane.0;
                    }
                    self.visible = pane.1;
                    self.options = options;
                    self.pinned_parents = pins
                        .iter()
                        .filter(|node| node.is_fs)
                        .map(|node| crate::app::ids::parent_of(&node.id))
                        .collect();
                    self.failed_images.clear();
                    self.tree.set_roots(roots, pins, desktop);
                    self.changed();
                }
                Reply::Children(id, reason, children) => {
                    self.pending_children.remove(&id);
                    self.tree.replace_children(&id, children);
                    if let Some(row) = self.row_of(&id) {
                        if !self.follow_pending && matches!(reason, ChildLoad::Expansion) {
                            self.reveal(row, self.tree.subtree_end(row));
                        }
                        if self.expanding_all.remove(&id) {
                            self.expand_all(row, worker);
                        }
                    }
                    self.changed();
                }
                Reply::Icons(images, failures) => {
                    let changed: HashSet<_> = images.iter().map(|(index, _)| *index).collect();
                    for (index, image) in images {
                        self.pending_images.remove(&index);
                        self.images.insert(index, image);
                    }
                    for index in failures {
                        self.pending_images.remove(&index);
                        self.failed_images.insert(index);
                    }
                    let rows: Vec<_> = self
                        .visible_rows()
                        .filter(|row| {
                            self.tree
                                .node(&self.tree.rows[*row].reference)
                                .is_some_and(|node| changed.contains(&node.node.icon))
                        })
                        .collect();
                    for row in rows {
                        self.dirty(Some(row));
                    }
                }
                Reply::Error(error) => {
                    eprintln!("native navigation: {error}");
                    self.pending_children.clear();
                    self.pending_images.clear();
                    self.follow_pending = false;
                }
                _ => {}
            }
        }
        self.top = self.top.min(self.maximum());
    }

    pub fn row_of(&self, id: &str) -> Option<usize> {
        self.tree.rows.iter().position(|row| {
            self.tree
                .node(&row.reference)
                .is_some_and(|node| crate::app::ids::same(&node.node.id, id))
        })
    }

    pub fn select(&mut self, row: usize, path: &str) {
        if let Some(reference) = self.tree.rows.get(row).map(|row| row.reference.clone()) {
            if let Some(node) = self.tree.node(&reference) {
                self.pick = Some(Pick {
                    reference,
                    id: node.node.id.clone(),
                    at: path.into(),
                });
                self.dirty(self.selected);
                self.selected = Some(row);
                self.dirty(Some(row));
            }
        }
    }

    pub fn sync(&mut self, path: &str, crumbs: &[crate::shell::listing::Crumb], worker: &Worker) {
        if self.followed != path {
            self.followed = path.into();
            if self.options.expand {
                self.follow_pending = true;
                self.manual_follow = false;
            }
        }
        if !self.options.expand && !self.manual_follow {
            self.follow_pending = false;
        }
        if self.follow_pending {
            let chain: Vec<String> = crumbs.iter().map(|crumb| crumb.id.clone()).collect();
            loop {
                let mode = if self.manual_follow {
                    crate::app::navtree::Follow::Current
                } else {
                    crate::app::navtree::Follow::Ancestors
                };
                let Some(step) = self.tree.follow(&chain, mode) else {
                    break;
                };
                let row = step.row;
                if !step.open {
                    if !self.pending_children.is_empty() {
                        break;
                    }
                    self.pick = None;
                    self.follow_pending = false;
                    self.reveal(
                        row,
                        if self.manual_follow {
                            self.tree.subtree_end(row)
                        } else {
                            row
                        },
                    );
                    break;
                }
                self.expand(row, worker);
                if !step.loaded {
                    break;
                }
            }
        }
        let selected = self.tree.picked_row(self.pick.as_ref(), path, crumbs);
        if selected != self.selected {
            self.dirty(self.selected);
            self.selected = selected;
            self.dirty(selected);
        }
        self.request_visible(worker);
    }

    pub fn expand(&mut self, row: usize, worker: &Worker) {
        let Some(reference) = self.tree.rows.get(row).map(|row| row.reference.clone()) else {
            return;
        };
        let Some(node) = self.tree.node_mut(&reference) else {
            return;
        };
        if !node.node.has_children || node.expanded {
            return;
        }
        node.expanded = true;
        let load = node.children.is_none().then(|| node.node.id.clone());
        self.tree.flatten();
        if let Some(id) = load {
            if self.pending_children.insert(id.clone()) {
                worker.send(Request::Children(id, ChildLoad::Expansion));
            }
        } else {
            self.reveal(row, self.tree.subtree_end(row));
        }
        self.changed();
    }

    pub fn collapse(&mut self, row: usize) -> Option<String> {
        let reference = self.tree.rows.get(row)?.reference.clone();
        let holds = self
            .selected
            .and_then(|selected| self.tree.rows.get(selected))
            .is_some_and(|selected| {
                crate::app::navtree::is_inside(&selected.reference, &reference)
            });
        let node = self.tree.node_mut(&reference)?;
        node.expanded = false;
        let id = node.node.id.clone();
        self.tree.flatten();
        self.top = self.top.min(self.maximum());
        self.changed();
        holds.then_some(id)
    }

    pub fn expand_all(&mut self, row: usize, worker: &Worker) {
        let Some(id) = self
            .tree
            .rows
            .get(row)
            .and_then(|row| self.tree.node(&row.reference))
            .map(|node| node.node.id.clone())
        else {
            return;
        };
        let mut pending = vec![id];
        while let Some(id) = pending.pop() {
            let Some(row) = self.row_of(&id) else {
                continue;
            };
            let Some(node) = self.tree.node(&self.tree.rows[row].reference) else {
                continue;
            };
            if !node.node.has_children {
                continue;
            }
            let children = node.children.as_ref().map(|children| {
                children
                    .iter()
                    .map(|child| child.node.id.clone())
                    .collect::<Vec<_>>()
            });
            self.expand(row, worker);
            if let Some(children) = children {
                pending.extend(children.into_iter().rev());
            } else {
                self.expanding_all.insert(id);
            }
        }
    }

    pub fn reveal(&mut self, from: usize, to: usize) {
        if let Some(top) = crate::app::navtree::reveal_top(from, to, self.top, self.page()) {
            self.top = top.min(self.maximum());
        }
        self.rehover();
    }

    pub fn scroll(&mut self, delta: isize) {
        self.top = (self.top as isize + delta).clamp(0, self.maximum() as isize) as usize;
        self.rehover();
    }
    pub fn wheel(&mut self, delta: i32) {
        let notches = crate::app::scroll::wheel_steps(&mut self.wheel, delta);
        self.scroll(-3 * notches as isize);
    }

    pub fn visible_rows(&self) -> std::ops::Range<usize> {
        let start = self.top.min(self.tree.rows.len());
        start
            ..(start + (self.height / self.metrics().0).ceil() as usize + 2)
                .min(self.tree.rows.len())
    }

    fn request_visible(&mut self, worker: &Worker) {
        if !self.visible {
            return;
        }
        let indices: Vec<_> = self
            .visible_rows()
            .filter_map(|row| {
                self.tree
                    .node(&self.tree.rows[row].reference)
                    .map(|node| node.node.icon)
            })
            .filter(|index| {
                !self.images.contains_key(index)
                    && !self.failed_images.contains(index)
                    && self.pending_images.insert(*index)
            })
            .collect();
        if !indices.is_empty() {
            worker.send(Request::Icons(indices));
        }
        if self.images.len() > 256 {
            let kept: HashSet<u32> = self
                .visible_rows()
                .filter_map(|row| {
                    self.tree
                        .node(&self.tree.rows[row].reference)
                        .map(|node| node.node.icon)
                })
                .collect();
            self.images.retain(|index, _| kept.contains(index));
        }
    }

    pub fn bar(&self) -> Option<super::scroll::Bar> {
        if self.maximum() == 0 || self.height < 34.0 {
            return None;
        }
        let track = self.height - 34.0;
        let thumb = (track * self.page() as f32 / self.tree.rows.len().max(1) as f32)
            .round()
            .clamp(17.0, track.max(17.0));
        Some(super::scroll::Bar {
            axis: super::scroll::Axis::Vertical,
            length: self.height,
            maximum: self.maximum() as f32,
            thumb_size: thumb,
            thumb_pos: 17.0 + (self.top as f32 / self.maximum() as f32 * (track - thumb)).round(),
            overlay: false,
        })
    }

    pub fn hit(&self, point: (f32, f32)) -> Option<Hit> {
        if !self.visible
            || point.0 < 0.0
            || point.0 >= self.left()
            || point.1 < super::details::top()
            || point.1 >= super::details::top() + self.height
        {
            return None;
        }
        if point.0 >= self.left() - 4.0 {
            return Some(Hit::Splitter);
        }
        let local = (point.0 - 2.0, point.1 - super::details::top());
        if local.0 >= self.width - 17.0 {
            if let Some(bar) = self.bar() {
                return Some(Hit::Bar(bar.hit(local.1).part));
            }
        }
        let y = local.1 + self.offset() - self.metrics().1;
        if y < 0.0 {
            return Some(Hit::Background);
        }
        let row = (y / self.metrics().0) as usize;
        let Some(item) = self.tree.rows.get(row) else {
            return Some(Hit::Background);
        };
        let Some(node) = self.tree.node(&item.reference) else {
            return Some(Hit::Background);
        };
        let expando = item.depth as f32 * 8.0 + 3.0 + if self.compact { 0.0 } else { 8.0 };
        Some(
            if node.node.has_children && local.0 >= expando && local.0 < expando + 16.0 {
                Hit::Expando(row)
            } else {
                Hit::Row(row)
            },
        )
    }
}
