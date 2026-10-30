use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Shell::IShellFolder;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::app::{columns, sort};
use crate::shell::{self, icons::Rgba, listing::ShellEntry};

pub const READY: u32 = WM_APP + 1;

pub fn discovery(path: &str) -> bool {
    crate::app::ids::same(path, crate::app::ids::NETWORK) || shell::network::is_computer(path)
}

#[derive(Clone, Default)]
pub struct SearchState {
    current: Arc<std::sync::atomic::AtomicBool>,
    workers: Arc<std::sync::atomic::AtomicUsize>,
    first_hit: Arc<std::sync::OnceLock<std::time::Instant>>,
}

impl SearchState {
    pub fn busy(&self) -> bool {
        self.current.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn workers(&self) -> usize {
        self.workers.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn cancel(&self) {
        self.current
            .store(false, std::sync::atomic::Ordering::Release);
    }

    pub fn hit_elapsed(&self) -> std::time::Duration {
        self.first_hit
            .get()
            .map_or(std::time::Duration::from_millis(3550), |at| at.elapsed())
    }
}

struct SearchThread {
    state: SearchState,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SearchThread {
    fn discover(
        path: String,
        epoch: u64,
        state: SearchState,
        requests: mpsc::Sender<Request>,
    ) -> std::io::Result<Self> {
        let running = state.clone();
        running
            .workers
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let thread = std::thread::Builder::new()
            .name("native-discovery".into())
            .spawn(move || {
                shell::com_init();
                let started=std::time::Instant::now();
                let probe=std::env::var("SW_DISCOVERY_PROBE").as_deref()==Ok("1");
                let mut count=0;
                if probe {eprintln!("native discovery started {epoch} {path}");}
                let result = shell::Pidl::parse(&path).and_then(|parent| {
                    shell::listing::for_each_pidl(&parent, |entry| {
                        if !running.busy() {
                            return false;
                        }
                        count+=1;
                        if requests
                            .send(Request::DiscoveryBatch(epoch, entry))
                            .is_err()
                        {
                            running.cancel();
                            return false;
                        }
                        true
                    })
                });
                if probe {eprintln!("native discovery finished {epoch} {}ms items={count} current={} result={result:?}",started.elapsed().as_millis(),running.busy());}
                running
                    .workers
                    .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                if let Err(error) = requests.send(Request::DiscoveryFinished(epoch, result)) {
                    eprintln!("native discovery completion: {error}");
                }
            });
        match thread {
            Ok(thread) => Ok(Self {
                state,
                thread: Some(thread),
            }),
            Err(error) => {
                state
                    .workers
                    .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                Err(error)
            }
        }
    }

    fn start(
        path: String,
        options: shell::search::Options,
        state: SearchState,
        requests: mpsc::Sender<Request>,
    ) -> std::io::Result<Self> {
        let running = state.clone();
        running
            .workers
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let thread = std::thread::Builder::new()
            .name("native-search".into())
            .spawn(move || {
                shell::com_init();
                let result = match crate::app::ids::search_parts(&path) {
                    Some((scope, query)) => shell::search::stream(
                        scope,
                        query,
                        options,
                        || running.busy(),
                        |batch| {
                            if requests.send(Request::SearchBatch(batch)).is_err() {
                                running.cancel();
                            }
                        },
                    ),
                    None => Err("Invalid search location".into()),
                };
                running
                    .workers
                    .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                if let Err(error) = requests.send(Request::SearchFinished(result)) {
                    eprintln!("native search completion: {error}");
                }
            });
        match thread {
            Ok(thread) => Ok(Self {
                state,
                thread: Some(thread),
            }),
            Err(error) => {
                state
                    .workers
                    .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                Err(error)
            }
        }
    }
}

impl Drop for SearchThread {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.state.cancel();
            if thread.join().is_err() {
                eprintln!("native search worker panicked");
            }
        }
    }
}

#[derive(Clone)]
pub struct Folder {
    pub crumbs: Arc<Vec<shell::listing::Crumb>>,
    pub title: String,
    pub icon: Rgba,
    pub entries: Arc<Vec<ShellEntry>>,
    pub order: Arc<Vec<usize>>,
    pub headers: Arc<Vec<String>>,
    pub collapsed: HashSet<String>,
    pub layout: Arc<crate::app::group::DetailsLayout>,
    pub grid: Option<Arc<super::grid::Model>>,
    pub gallery_filter: Option<String>,
    pub home_filter: u8,
    pub filter: Option<crate::app::filter::Filter>,
    pub network_notice: Option<String>,
    pub library_origins: Arc<Vec<shell::library::Origin>>,
    pub columns: Vec<columns::Col>,
    pub view: crate::app::view::ResolvedView,
    pub settings: shell::listing::ExplorerSettings,
    pub panes: (bool, bool),
    pub pane_width: f32,
}

pub struct Row {
    pub index: usize,
    pub cells: Vec<String>,
    pub icon: u32,
    pub image: Option<Arc<Rgba>>,
    pub picture: Option<super::grid::Picture>,
}

impl Folder {
    pub fn awaiting_grid(&self) -> bool {
        self.grid.is_none()
            && self.view.mode != 6
            && !self
                .crumbs
                .last()
                .is_some_and(|crumb| crate::app::ids::same(&crumb.id, crate::app::ids::HOME))
    }

    pub fn set_collapsed(&mut self, collapsed: HashSet<String>) {
        let grid = self.grid.take();
        self.collapsed = collapsed;
        self.rebuild_layout();
        if let Some(mut grid) = grid {
            let model = Arc::make_mut(&mut grid);
            model.arrange(self);
            model.bind_layout(self.layout.clone());
            self.grid = Some(grid);
        }
    }

    pub fn library_grouped(&self) -> bool {
        crate::app::library_view::grouped(&self.library_origins, self.view.group)
    }

    pub fn library_ranges(&self) -> Vec<(String, usize, usize)> {
        crate::app::library_view::ranges(
            &self.entries,
            &self.order,
            &self.library_origins,
            self.view.group,
        )
    }

    pub fn rebuild_layout(&mut self) {
        self.grid = None;
        if self.library_grouped() {
            self.layout = Arc::new(crate::app::library_view::details(
                self.order.len(),
                &self.library_ranges(),
                &self.collapsed,
                if self.settings.compact { 21.0 } else { 28.0 },
            ));
            return;
        }
        self.layout = Arc::new(crate::app::group::DetailsLayout::new(
            self.order.len(),
            &self.headers,
            &self.collapsed,
            if self.settings.compact { 21.0 } else { 28.0 },
            if self.settings.compact { 22.0 } else { 24.0 },
        ));
    }
}

struct Reveal {
    path: String,
    view: super::details::Viewport,
    offset: f32,
}

enum Request {
    Pane(
        u64,
        crate::app::pane_model::Kind,
        crate::app::pane_model::Selection,
        u32,
    ),
    DiscoveryBatch(u64, ShellEntry),
    DiscoveryFinished(u64, Result<(), String>),
    HomeFilter(u64, u8),
    Filter(u64, Option<crate::app::filter::Filter>),
    FilterMenu(u64, sort::SortKey),
    SearchBatch(Vec<ShellEntry>),
    SearchFinished(Result<(), String>),
    Viewport(std::ops::Range<usize>),
    Retry(u64, Vec<usize>),
    GalleryFilter(u64, Option<String>),
    Mode(u64, i32, u32),
    Grid(
        u64,
        (f32, f32),
        i32,
        u32,
        Arc<crate::app::group::DetailsLayout>,
    ),
    GridLabels(u64),
    Stop,
    Watch(notify::Event),
    WatchError(String),
    Rows(u64, Vec<usize>),
    Change(u64, Vec<notify::Event>, std::ops::Range<usize>),
    Refresh(u64, std::ops::Range<usize>, Option<Reveal>),
    Group(
        u64,
        Option<crate::app::group::Group>,
        std::ops::Range<usize>,
        Option<Reveal>,
    ),
    Collapse(HashSet<String>),
    Toggle(shell::listing::FolderOption),
    Layout(Vec<columns::Col>),
    Settings(shell::listing::ExplorerSettings, (bool, bool)),
    Sort(u64, sort::Sort, std::ops::Range<usize>, Option<Reveal>),
    Columns(
        u64,
        Vec<columns::Col>,
        std::ops::Range<usize>,
        Option<Reveal>,
    ),
    Fit(
        u64,
        Vec<columns::Col>,
        std::ops::Range<usize>,
        Vec<usize>,
        f32,
    ),
    Save(String, shell::viewstate::ViewState),
}

enum Update {
    FilterMenu(u64, sort::SortKey, Vec<String>),
    Pane(u64, Arc<crate::app::pane_model::Model>),
    SearchBatch(u64, Arc<Folder>, (usize, usize)),
    Grid(u64, Arc<super::grid::Model>),
    Loaded(Arc<Folder>),
    Event(notify::Event),
    Changed(u64, Arc<Folder>, Vec<Row>, (usize, usize)),
    WatchError(String),
    Rows(u64, Vec<Row>),
    Order(u64, Arc<Vec<usize>>, Arc<Vec<String>>, Vec<Row>),
    Fit(u64, Vec<(usize, f32)>),
    SearchFinished(Result<(), String>),
    Error(String),
}

#[derive(Clone, Copy, PartialEq)]
enum ViewChange {
    Stable,
    Sort,
    Rebuild,
}

pub struct Worker {
    jobs: Option<mpsc::Sender<Job>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    pub fn start() -> std::io::Result<Self> {
        let (jobs, receiver) = mpsc::channel::<Job>();
        let thread = std::thread::Builder::new()
            .name("native-list".into())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    job.run();
                }
            })?;
        Ok(Self {
            jobs: Some(jobs),
            thread: Some(thread),
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.jobs.take();
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                eprintln!("native list worker panicked");
            }
        }
    }
}

struct Job {
    address: usize,
    path: String,
    session: Option<crate::app::view::ResolvedView>,
    initial: Option<Arc<Folder>>,
    updates_tx: mpsc::Sender<Update>,
    requests_rx: mpsc::Receiver<Request>,
    watched_requests: mpsc::Sender<Request>,
    alive: std::sync::Weak<()>,
    search: SearchState,
    search_options: shell::search::Options,
}

impl Job {
    fn run(self) {
        let Self {
            address,
            path,
            session,
            initial,
            updates_tx,
            requests_rx,
            watched_requests,
            alive,
            search,
            search_options,
        } = self;
        if alive.upgrade().is_none() {
            return;
        }
        let send = |update| {
            if updates_tx.send(update).is_err() {
                return false;
            }
            if let Err(error) =
                unsafe { PostMessageW(Some(HWND(address as _)), READY, WPARAM(0), LPARAM(0)) }
            {
                eprintln!("native list notification: {error}");
                return false;
            }
            true
        };
        super::memory::start_shell();
        let cached = initial.is_some();
        let mut folder =
            match initial.map_or_else(|| load(&path, session), |folder| Ok((*folder).clone())) {
                Ok(folder) => folder,
                Err(error) => {
                    send(Update::Error(error));
                    return;
                }
            };
        if !send(Update::Loaded(Arc::new(folder.clone()))) {
            return;
        }
        let watch_path = if crate::app::ids::is_search(&path) {
            String::new()
        } else {
            match shell::Pidl::parse(&path) {
                Ok(parent) => parent.name(windows::Win32::UI::Shell::SIGDN_FILESYSPATH),
                Err(error) => {
                    send(Update::WatchError(error));
                    String::new()
                }
            }
        };
        let watch_paths = if folder.library_origins.is_empty() {
            vec![watch_path.clone()]
        } else {
            folder
                .library_origins
                .iter()
                .map(|origin| origin.path.clone())
                .collect()
        };
        let _watcher = if watch_paths
            .iter()
            .any(|path| std::path::Path::new(path).is_dir())
        {
            let requests = watched_requests.clone();
            match crate::watch::listen(&watch_paths, move |event| {
                let request = match event {
                    Ok(event) => Request::Watch(event),
                    Err(error) => Request::WatchError(error.to_string()),
                };
                if let Err(error) = requests.send(request) {
                    eprintln!("native watcher request: {error}");
                }
            }) {
                Ok(watcher) => Some(watcher),
                Err(error) => {
                    send(Update::WatchError(error.to_string()));
                    None
                }
            }
        } else {
            None
        };
        let children = folder
            .entries
            .iter()
            .filter(|e| !e.child.is_empty())
            .map(|e| (e.path.clone(), e.child.clone()))
            .collect();
        let mut items = shell::columns::Items::new(&path, children);
        let mut icons = RowIcons::default();
        let mut generation = 0;
        let mut counts = (usize::from(!cached), 0);
        let mut queue = std::collections::VecDeque::new();
        let mut refresh_cached = cached;
        let mut retries = super::grid::Retries::default();
        if cached && folder.order.is_empty() {
            queue.push_back(Request::Refresh(0, 0..32, None));
            refresh_cached = false;
        }
        let mut discovered = HashSet::new();
        let mut pending_discovery = None;
        let mut search_thread = if discovery(&path) && !cached {
            match SearchThread::discover(
                path.clone(),
                generation,
                search.clone(),
                watched_requests.clone(),
            ) {
                Ok(thread) => Some(thread),
                Err(error) => {
                    search.cancel();
                    send(Update::WatchError(error.to_string()));
                    None
                }
            }
        } else if crate::app::ids::is_search(&path) && !cached {
            match SearchThread::start(
                path.clone(),
                search_options,
                search.clone(),
                watched_requests.clone(),
            ) {
                Ok(thread) => Some(thread),
                Err(error) => {
                    search.cancel();
                    send(Update::SearchFinished(Err(error.to_string())));
                    None
                }
            }
        } else {
            None
        };
        let mut renames = crate::watch::Renames::default();
        loop {
            let request = if let Some(request) = queue.pop_front() {
                request
            } else if renames.waiting() || retries.wait().is_some() {
                let wait = retries.wait().unwrap_or(std::time::Duration::MAX);
                let wait = if renames.waiting() {
                    wait.min(std::time::Duration::from_millis(50))
                } else {
                    wait
                };
                match requests_rx.recv_timeout(wait) {
                    Ok(request) => request,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Some(event) = renames.flush() {
                            send(Update::Event(event));
                        }
                        let due = retries.due();
                        if !due.is_empty() {
                            queue.push_back(Request::Retry(generation, due));
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match requests_rx.recv() {
                    Ok(request) => request,
                    Err(_) => break,
                }
            };
            match request {
                Request::FilterMenu(token, key) => {
                    let today = crate::app::group::Today::now();
                    let mut buckets = Vec::new();
                    for entry in folder.entries.iter() {
                        let value = key
                            .property()
                            .map(|key| shell::columns::sort_value(&items, &entry.path, &key));
                        let bucket = crate::app::group::bucket(key, entry, &today, value.as_ref());
                        if !buckets.contains(&bucket) {
                            buckets.push(bucket);
                        }
                    }
                    let descending = crate::app::group::Group::new(key).descending;
                    buckets.sort_by(|left, right| {
                        if descending {
                            right.cmp(left)
                        } else {
                            left.cmp(right)
                        }
                    });
                    send(Update::FilterMenu(
                        token,
                        key,
                        buckets.into_iter().map(|bucket| bucket.label).collect(),
                    ));
                }
                Request::Filter(epoch, filter) => {
                    generation = epoch;
                    folder.filter = filter;
                    arrange(&mut folder, &items);
                    send(Update::Changed(
                        epoch,
                        Arc::new(folder.clone()),
                        Vec::new(),
                        counts,
                    ));
                }
                Request::HomeFilter(epoch, kind) => {
                    generation = epoch;
                    folder.home_filter = kind;
                    arrange(&mut folder, &items);
                    send(Update::Changed(
                        epoch,
                        Arc::new(folder.clone()),
                        Vec::new(),
                        counts,
                    ));
                }
                Request::Pane(token, kind, selection, width) => {
                    send(Update::Pane(
                        token,
                        Arc::new(crate::app::pane_model::load(kind, selection, width)),
                    ));
                }
                Request::DiscoveryBatch(epoch, entry) => {
                    if epoch != generation || !search.busy() {
                        continue;
                    }
                    discovered.insert(entry.path.clone());
                    let entries = Arc::make_mut(&mut folder.entries);
                    if let Some(index) = entries
                        .iter()
                        .position(|existing| crate::app::ids::same(&existing.path, &entry.path))
                    {
                        entries[index] = entry;
                    } else {
                        entries.push(entry);
                    }
                    let children = entries
                        .iter()
                        .filter(|entry| !entry.child.is_empty())
                        .map(|entry| (entry.path.clone(), entry.child.clone()))
                        .collect();
                    items = shell::columns::Items::new(&path, children);
                    arrange(&mut folder, &items);
                    send(Update::Changed(
                        epoch,
                        Arc::new(folder.clone()),
                        Vec::new(),
                        counts,
                    ));
                }
                Request::DiscoveryFinished(epoch, result) => {
                    let completed = search.busy();
                    drop(search_thread.take());
                    if let Some(request) = pending_discovery.take() {
                        queue.push_front(request);
                        continue;
                    }
                    if epoch != generation {
                        continue;
                    }
                    if result.is_ok() && completed {
                        Arc::make_mut(&mut folder.entries)
                            .retain(|entry| discovered.contains(&entry.path));
                        arrange(&mut folder, &items);
                    }
                    search.cancel();
                    if let Err(error) = result {
                        send(Update::WatchError(error));
                    }
                    send(Update::Changed(
                        epoch,
                        Arc::new(folder.clone()),
                        Vec::new(),
                        counts,
                    ));
                }
                Request::SearchBatch(batch) => {
                    if !search.current.load(std::sync::atomic::Ordering::Acquire) {
                        continue;
                    }
                    let _ = search.first_hit.set(std::time::Instant::now());
                    Arc::make_mut(&mut folder.entries).extend(batch);
                    let children = folder
                        .entries
                        .iter()
                        .filter(|entry| !entry.child.is_empty())
                        .map(|entry| (entry.path.clone(), entry.child.clone()))
                        .collect();
                    items = shell::columns::Items::new(&path, children);
                    arrange(&mut folder, &items);
                    send(Update::SearchBatch(
                        generation,
                        Arc::new(folder.clone()),
                        counts,
                    ));
                }
                Request::SearchFinished(result) => {
                    if let Some(mut worker) = search_thread.take() {
                        if worker
                            .thread
                            .take()
                            .is_some_and(|thread| thread.join().is_err())
                        {
                            send(Update::SearchFinished(Err(
                                "Native search worker panicked".into()
                            )));
                            continue;
                        }
                    }
                    send(Update::SearchFinished(result));
                }
                Request::Viewport(range) => retries.retain(range),
                Request::Retry(epoch, indices) => {
                    if epoch != generation {
                        continue;
                    }
                    let rows = ready_rows(&folder, indices, &items, &mut icons);
                    for row in &rows {
                        if row
                            .picture
                            .as_ref()
                            .is_some_and(|picture| picture.image.is_none())
                        {
                            retries.missing(row.index);
                        } else {
                            retries.complete(row.index);
                        }
                    }
                    send(Update::Rows(epoch, rows));
                }
                Request::Stop => break,
                Request::GalleryFilter(epoch, root) => {
                    retries.clear();
                    generation = epoch;
                    folder.gallery_filter = root;
                    arrange(&mut folder, &items);
                    send(Update::Changed(
                        epoch,
                        Arc::new(folder.clone()),
                        Vec::new(),
                        counts,
                    ));
                }
                Request::Mode(epoch, mode, size) => {
                    retries.clear();
                    generation = epoch;
                    folder.view.mode = mode;
                    folder.view.icon_size = size;
                    folder.grid = None;
                    icons.clear();
                    if folder.crumbs.last().is_some_and(|crumb| {
                        crate::app::ids::same(&crumb.id, crate::app::ids::THIS_PC)
                    }) {
                        arrange(&mut folder, &items);
                        send(Update::Changed(
                            epoch,
                            Arc::new(folder.clone()),
                            Vec::new(),
                            counts,
                        ));
                    }
                }
                Request::Grid(epoch, size, mode, icon_size, layout) => {
                    if epoch != generation {
                        continue;
                    }
                    folder.view.mode = mode;
                    folder.view.icon_size = icon_size;
                    folder.grid = super::grid::Model::new(&folder, size).map(|mut model| {
                        model.bind_layout(layout);
                        Arc::new(model)
                    });
                    icons.clear();
                    if let Some(model) = &folder.grid {
                        send(Update::Grid(epoch, model.clone()));
                        if model.measured < folder.order.len() {
                            queue.push_back(Request::GridLabels(epoch));
                        }
                    }
                }
                Request::GridLabels(epoch) => {
                    if epoch != generation {
                        continue;
                    }
                    let Some(mut model) = folder.grid.take() else {
                        continue;
                    };
                    Arc::make_mut(&mut model).measure(&folder, 128);
                    let more = model.measured < folder.order.len();
                    folder.grid = Some(model.clone());
                    if !send(Update::Grid(epoch, model)) {
                        break;
                    }
                    while let Ok(next) = requests_rx.try_recv() {
                        queue.push_back(next);
                    }
                    if more {
                        queue.push_back(Request::GridLabels(epoch));
                    }
                }
                Request::Watch(event) => {
                    for event in renames.push(event) {
                        send(Update::Event(event));
                    }
                }
                Request::WatchError(error) => {
                    send(Update::WatchError(error));
                }
                Request::Rows(epoch, mut indices) => {
                    while let Ok(next) = requests_rx.try_recv() {
                        match next {
                            Request::Rows(other, latest) if other == epoch => indices = latest,
                            other => queue.push_back(other),
                        }
                    }
                    if epoch == generation {
                        let range = indices.iter().copied().min().unwrap_or(0)
                            ..indices.iter().copied().max().map_or(0, |index| index + 1);
                        if folder.grid.is_some() && indices.len() > 8 {
                            let remaining = indices.split_off(8);
                            queue.push_back(Request::Rows(epoch, remaining));
                        }
                        let rows = ready_rows(&folder, indices, &items, &mut icons);
                        if folder
                            .grid
                            .as_ref()
                            .is_some_and(|grid| grid.kind == super::grid::Kind::Gallery)
                        {
                            for row in &rows {
                                if row
                                    .picture
                                    .as_ref()
                                    .is_some_and(|picture| picture.image.is_none())
                                {
                                    retries.missing(row.index);
                                } else {
                                    retries.complete(row.index);
                                }
                            }
                        }
                        send(Update::Rows(epoch, rows));
                        if refresh_cached {
                            queue.push_front(Request::Refresh(epoch, range, None));
                            refresh_cached = false;
                        }
                    }
                }
                Request::Change(epoch, _events, range) if !folder.library_origins.is_empty() => {
                    queue.push_front(Request::Refresh(epoch, range, None));
                }
                Request::Change(epoch, events, range) => {
                    generation = epoch;
                    let started = std::time::Instant::now();
                    let previous = folder
                        .order
                        .iter()
                        .map(|&index| folder.entries[index].path.clone())
                        .collect::<Vec<_>>();
                    let renames = events
                        .iter()
                        .filter(|event| {
                            matches!(
                                event.kind,
                                notify::EventKind::Modify(notify::event::ModifyKind::Name(
                                    notify::event::RenameMode::Both
                                ))
                            ) && event.paths.len() == 2
                        })
                        .map(|event| {
                            (
                                event.paths[0].to_string_lossy().into_owned(),
                                event.paths[1].to_string_lossy().into_owned(),
                            )
                        })
                        .collect::<Vec<_>>();
                    let mut seen = HashSet::new();
                    let paths = events
                        .into_iter()
                        .flat_map(|event| event.paths)
                        .filter(|path| seen.insert(path.clone()))
                        .collect::<Vec<_>>();
                    let mut entries = folder.entries.as_ref().clone();
                    for changed_path in paths {
                        if changed_path.parent().is_none_or(|parent| {
                            !crate::app::ids::same(&parent.to_string_lossy(), &watch_path)
                        }) {
                            continue;
                        }
                        let name = changed_path.to_string_lossy();
                        counts.1 += 1;
                        match shell::listing::read_file(&name) {
                            Ok(entry) => {
                                let existing = entries
                                    .iter()
                                    .position(|entry| crate::app::ids::same(&entry.path, &name));
                                match (existing, entry) {
                                    (Some(index), Some(entry)) => entries[index] = entry,
                                    (Some(index), None) => {
                                        entries.remove(index);
                                    }
                                    (None, Some(entry)) => entries.push(entry),
                                    (None, None) => {}
                                }
                            }
                            Err(error) => {
                                send(Update::WatchError(error));
                            }
                        }
                    }
                    folder.entries = Arc::new(entries);
                    icons.clear();
                    let order = sort::updated_order(&previous, &folder.entries, &renames);
                    group_order(&mut folder, &items, order);
                    let range =
                        range.start.min(folder.order.len())..range.end.min(folder.order.len());
                    let rows = ready_rows(&folder, range.collect(), &items, &mut icons);
                    send(Update::Changed(
                        epoch,
                        Arc::new(folder.clone()),
                        rows,
                        counts,
                    ));
                    super::memory::work("change-worker", started);
                }
                Request::Refresh(epoch, range, reveal) if discovery(&path) => {
                    if search_thread.is_some() {
                        search.cancel();
                        pending_discovery = Some(Request::Refresh(epoch, range, reveal));
                        continue;
                    }
                    generation = epoch;
                    folder.network_notice = network_notice(&path);
                    discovered.clear();
                    icons.clear();
                    search
                        .current
                        .store(true, std::sync::atomic::Ordering::Release);
                    match SearchThread::discover(
                        path.clone(),
                        epoch,
                        search.clone(),
                        watched_requests.clone(),
                    ) {
                        Ok(thread) => search_thread = Some(thread),
                        Err(error) => {
                            search.cancel();
                            send(Update::WatchError(error.to_string()));
                        }
                    }
                    send(Update::Changed(
                        epoch,
                        Arc::new(folder.clone()),
                        Vec::new(),
                        counts,
                    ));
                }
                Request::Refresh(epoch, range, reveal) => {
                    generation = epoch;
                    match load(&path, Some(folder.view.clone())) {
                        Ok(mut updated) => {
                            counts.0 += 1;
                            updated.collapsed = folder.collapsed.clone();
                            updated.filter = folder.filter.clone();
                            updated.home_filter = folder.home_filter;
                            updated.gallery_filter = folder.gallery_filter.clone();
                            updated.rebuild_layout();
                            folder = updated;
                            let children = folder
                                .entries
                                .iter()
                                .filter(|entry| !entry.child.is_empty())
                                .map(|entry| (entry.path.clone(), entry.child.clone()))
                                .collect();
                            items = shell::columns::Items::new(&path, children);
                            arrange(&mut folder, &items);
                            icons.clear();
                            let range = reveal_range(&folder, range, reveal);
                            let rows = ready_rows(&folder, range.collect(), &items, &mut icons);
                            send(Update::Changed(
                                epoch,
                                Arc::new(folder.clone()),
                                rows,
                                counts,
                            ));
                        }
                        Err(error) => {
                            send(Update::WatchError(error));
                        }
                    }
                }
                Request::Sort(epoch, primary, range, reveal) => {
                    let started = std::time::Instant::now();
                    generation = epoch;
                    folder.view.sort = primary;
                    folder.view.extra_sorts.clear();
                    folder.view.search_ranked = false;
                    arrange(&mut folder, &items);
                    let range = reveal_range(&folder, range, reveal);
                    let rows = ready_rows(&folder, range.collect(), &items, &mut icons);
                    super::memory::work("sort-worker", started);
                    send(Update::Order(
                        epoch,
                        folder.order.clone(),
                        folder.headers.clone(),
                        rows,
                    ));
                }
                Request::Group(epoch, group, range, reveal) => {
                    generation = epoch;
                    folder.view.group = group;
                    folder.collapsed.clear();
                    arrange(&mut folder, &items);
                    let range = reveal_range(&folder, range, reveal);
                    send(Update::Order(
                        epoch,
                        folder.order.clone(),
                        folder.headers.clone(),
                        ready_rows(&folder, range.collect(), &items, &mut icons),
                    ));
                }
                Request::Toggle(option) => {
                    let settings = shell::listing::settings();
                    let current = match option {
                        shell::listing::FolderOption::Compact => settings.compact,
                        shell::listing::FolderOption::CheckBoxes => settings.check_boxes,
                        shell::listing::FolderOption::HiddenItems => settings.show_hidden,
                        shell::listing::FolderOption::Extensions => !settings.hide_extensions,
                    };
                    shell::listing::set_option(option, !current);
                }
                Request::Collapse(collapsed) => {
                    folder.set_collapsed(collapsed);
                }
                Request::Settings(settings, panes) => {
                    folder.settings = settings;
                    folder.panes = panes;
                    folder.rebuild_layout();
                }
                Request::Layout(cols) => {
                    folder.view.columns = cols.iter().map(|col| (col.key, col.width)).collect();
                    folder.columns = cols;
                }
                Request::Columns(epoch, cols, range, reveal) => {
                    generation = epoch;
                    folder.view.columns = cols.iter().map(|col| (col.key, col.width)).collect();
                    folder.columns = cols;
                    let range = reveal_range(&folder, range, reveal);
                    send(Update::Order(
                        epoch,
                        folder.order.clone(),
                        folder.headers.clone(),
                        ready_rows(&folder, range.collect(), &items, &mut icons),
                    ));
                }
                Request::Fit(epoch, cols, range, indices, indent) if epoch == generation => {
                    let keys = cols
                        .iter()
                        .map(|column| column.key)
                        .filter(columns::needs_property)
                        .collect::<Vec<_>>();
                    let values = range
                        .filter_map(|row| folder.order.get(row))
                        .map(|&i| read_cells(&folder, &folder.entries[i], &cols, &keys, &items))
                        .collect::<Vec<_>>();
                    let widths = indices
                        .into_iter()
                        .filter_map(|i| {
                            let col = cols.get(i)?;
                            let header = shell::columns::widest([col.label.as_str()]);
                            let widest = shell::columns::widest(
                                values.iter().filter_map(|r| r.get(i)).map(String::as_str),
                            );
                            Some((
                                i,
                                crate::app::header::fit_width(
                                    header,
                                    widest,
                                    if i == 0 { indent } else { 6.0 },
                                ),
                            ))
                        })
                        .collect();
                    send(Update::Fit(epoch, widths));
                }
                Request::Save(path, view) => shell::viewstate::write(&path, &view),
                _ => {}
            }
        }
    }
}

pub struct List {
    _alive: Arc<()>,
    pub navigation_width: f32,
    pub header: super::header::State,
    pub interaction: super::interaction::State,
    pub folder: Option<Arc<Folder>>,
    pub rows: HashMap<usize, Row>,
    pub cut: HashSet<String>,
    pub search: SearchState,
    pub pane_model: Option<(u64, Arc<crate::app::pane_model::Model>)>,
    pub enumerations: usize,
    pub entry_reads: usize,
    pub dirty: bool,
    pub layout_changed: bool,
    pub pending_selection: Option<crate::app::selection::Selection>,
    pub filter_menu: Option<(u64, sort::SortKey, Vec<String>)>,
    requested_filter: Option<crate::app::filter::Filter>,
    pending: HashSet<usize>,
    updates: mpsc::Receiver<Update>,
    requests: mpsc::Sender<Request>,
    needed: std::ops::Range<usize>,
    generation: u64,
    change: ViewChange,
    grid_request: Option<(i32, u32, u32, u32, usize)>,
}

impl Drop for List {
    fn drop(&mut self) {
        self.search.cancel();
        if let Err(error) = self.requests.send(Request::Stop) {
            eprintln!("native list stop: {error}");
        }
    }
}

impl List {
    pub fn start(
        worker: &Worker,
        hwnd: HWND,
        path: String,
        session: Option<crate::app::view::ResolvedView>,
    ) -> Result<Self, String> {
        Self::begin(worker, hwnd, path, session, None, Default::default())
    }

    pub fn resume(
        worker: &Worker,
        hwnd: HWND,
        path: String,
        folder: Arc<Folder>,
    ) -> Result<Self, String> {
        Self::begin(worker, hwnd, path, None, Some(folder), Default::default())
    }

    pub fn search_start(
        worker: &Worker,
        hwnd: HWND,
        path: String,
        session: Option<crate::app::view::ResolvedView>,
        options: shell::search::Options,
    ) -> Result<Self, String> {
        Self::begin(worker, hwnd, path, session, None, options)
    }

    fn begin(
        worker: &Worker,
        hwnd: HWND,
        path: String,
        session: Option<crate::app::view::ResolvedView>,
        initial: Option<Arc<Folder>>,
        search_options: shell::search::Options,
    ) -> Result<Self, String> {
        let (updates_tx, updates) = mpsc::channel();
        let (requests, requests_rx) = mpsc::channel::<Request>();
        let address = hwnd.0 as usize;
        let watched_requests = requests.clone();
        let alive = Arc::new(());
        let requested_filter = initial.as_ref().and_then(|folder| folder.filter.clone());
        let search = SearchState::default();
        search.current.store(
            (crate::app::ids::is_search(&path) || discovery(&path)) && initial.is_none(),
            std::sync::atomic::Ordering::Release,
        );
        worker
            .jobs
            .as_ref()
            .ok_or("native list worker stopped")?
            .send(Job {
                address,
                path,
                session,
                initial,
                updates_tx,
                requests_rx,
                watched_requests,
                alive: Arc::downgrade(&alive),
                search: search.clone(),
                search_options,
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            _alive: alive,
            pane_model: None,
            search: search.clone(),
            navigation_width: 202.0,
            header: Default::default(),
            interaction: Default::default(),
            folder: None,
            rows: HashMap::new(),
            cut: HashSet::new(),
            enumerations: 1,
            entry_reads: 0,
            dirty: false,
            layout_changed: false,
            pending_selection: None,
            filter_menu: None,
            requested_filter,
            pending: HashSet::new(),
            updates,
            requests,
            needed: 0..0,
            generation: 0,
            change: ViewChange::Stable,
            grid_request: None,
        })
    }

    fn send(&self, request: Request) -> Result<(), String> {
        self.requests
            .send(request)
            .map_err(|error| format!("native list worker stopped: {error}"))
    }

    pub fn prepare_grid(&mut self, size: (f32, f32)) -> Result<(), String> {
        let Some(folder) = &self.folder else {
            return Ok(());
        };
        let path = folder.crumbs.last().map_or("", |crumb| crumb.id.as_str());
        if folder.view.mode == 6 && !crate::app::ids::same(path, crate::app::ids::GALLERY)
            || crate::app::ids::same(path, crate::app::ids::HOME)
        {
            return Ok(());
        }
        let key = (
            folder.view.mode,
            folder.view.icon_size,
            size.0.to_bits(),
            size.1.to_bits(),
            Arc::as_ptr(&folder.layout) as usize,
        );
        if self.grid_request == Some(key) {
            return Ok(());
        }
        self.grid_request = Some(key);
        self.send(Request::Grid(
            self.generation,
            size,
            key.0,
            key.1,
            folder.layout.clone(),
        ))
    }

    pub fn filter_menu(&self, token: u64, key: sort::SortKey) -> Result<(), String> {
        self.send(Request::FilterMenu(token, key))
    }

    pub fn set_filter(&mut self, filter: Option<crate::app::filter::Filter>) -> Result<(), String> {
        if self.requested_filter == filter {
            return Ok(());
        }
        self.requested_filter = filter.clone();
        self.generation += 1;
        self.pending.clear();
        self.rows.clear();
        self.send(Request::Filter(self.generation, filter))
    }

    pub fn home_filter(&mut self, kind: u8) -> Result<(), String> {
        self.generation += 1;
        self.pending.clear();
        self.rows.clear();
        self.interaction.selection = Default::default();
        self.send(Request::HomeFilter(self.generation, kind))
    }

    pub fn set_mode(&mut self, mode: i32, size: u32) -> Result<(), String> {
        let Some(folder) = self.folder.as_mut() else {
            return Ok(());
        };
        let path = folder.crumbs.last().map_or("", |crumb| crumb.id.as_str());
        if crate::app::ids::same(path, crate::app::ids::HOME) {
            return Ok(());
        }
        let folder = Arc::make_mut(folder);
        folder.view.mode = mode;
        folder.view.icon_size = size;
        if mode == 6 {
            folder.grid = None;
        }
        self.grid_request = None;
        if mode == 6 {
            self.rows.clear();
        }
        self.pending.clear();
        self.dirty = true;
        self.layout_changed = true;
        self.interaction.mark_all();
        self.generation += 1;
        self.send(Request::Mode(self.generation, mode, size))
    }

    pub fn gallery_filter(&mut self, root: Option<String>) -> Result<(), String> {
        let Some(folder) = self.folder.as_mut() else {
            return Ok(());
        };
        let folder = Arc::make_mut(folder);
        folder.gallery_filter = root.clone();
        folder.grid = None;
        self.grid_request = None;
        self.generation += 1;
        self.pending.clear();
        self.rows.clear();
        self.interaction.selection = Default::default();
        self.interaction.mark_all();
        self.send(Request::GalleryFilter(self.generation, root))
    }

    pub fn sort(&mut self, sort: sort::Sort, scroll: &super::scroll::State) -> Result<(), String> {
        let Some(folder) = self.folder.as_mut() else {
            return Ok(());
        };
        let folder = Arc::make_mut(folder);
        folder.view.sort = sort;
        folder.view.extra_sorts.clear();
        folder.view.search_ranked = false;
        self.dirty = true;
        self.generation += 1;
        self.pending.clear();
        self.change = ViewChange::Sort;
        let reveal = self.selected_reveal(scroll);
        self.send(Request::Sort(
            self.generation,
            sort,
            self.needed.clone(),
            reveal,
        ))
    }

    fn selected_reveal(&self, scroll: &super::scroll::State) -> Option<Reveal> {
        self.interaction
            .selection
            .focused
            .as_ref()
            .filter(|path| self.interaction.selection.selected.contains(*path))
            .zip(scroll.view)
            .map(|(path, view)| Reveal {
                path: path.clone(),
                view,
                offset: scroll.y,
            })
    }

    pub fn sync_settings(
        &self,
        settings: shell::listing::ExplorerSettings,
        panes: (bool, bool),
    ) -> Result<(), String> {
        self.send(Request::Settings(settings, panes))
    }

    pub fn toggle(&self, option: shell::listing::FolderOption) -> Result<(), String> {
        self.send(Request::Toggle(option))
    }

    pub fn set_group(
        &mut self,
        group: Option<crate::app::group::Group>,
        scroll: &super::scroll::State,
    ) -> Result<(), String> {
        let Some(folder) = self.folder.as_mut() else {
            return Ok(());
        };
        let folder = Arc::make_mut(folder);
        folder.view.group = group;
        folder.collapsed.clear();
        self.interaction.group_focus = None;
        self.generation += 1;
        self.pending.clear();
        self.dirty = true;
        self.change = ViewChange::Rebuild;
        let mut reveal = self.selected_reveal(scroll);
        if let Some(reveal) = &mut reveal {
            reveal.offset = 0.0;
        }
        self.send(Request::Group(
            self.generation,
            group,
            0..self.needed.len(),
            reveal,
        ))
    }

    pub fn expand(
        &mut self,
        label: &str,
        expansion: crate::app::group::Expansion,
    ) -> Result<(), String> {
        let Some(folder) = self.folder.as_mut() else {
            return Ok(());
        };
        let folder = Arc::make_mut(folder);
        match expansion {
            crate::app::group::Expansion::Expanded => {
                folder.collapsed.remove(label);
            }
            crate::app::group::Expansion::Collapsed => {
                folder.collapsed.insert(label.to_owned());
            }
        }
        folder.set_collapsed(folder.collapsed.clone());
        self.layout_changed = true;
        self.interaction.mark_all();
        let collapsed = folder.collapsed.clone();
        self.send(Request::Collapse(collapsed))
    }

    pub fn set_columns(
        &mut self,
        cols: Vec<columns::Col>,
        scroll: &super::scroll::State,
    ) -> Result<(), String> {
        let Some(folder) = self.folder.as_mut() else {
            return Ok(());
        };
        for row in self.rows.values_mut() {
            row.cells = cols
                .iter()
                .map(|col| {
                    folder
                        .columns
                        .iter()
                        .position(|c| c.key == col.key)
                        .and_then(|i| row.cells.get(i))
                        .cloned()
                        .unwrap_or_default()
                })
                .collect();
        }
        let folder = Arc::make_mut(folder);
        folder.columns = cols.clone();
        folder.view.columns = cols.iter().map(|c| (c.key, c.width)).collect();
        self.generation += 1;
        self.pending.clear();
        self.dirty = true;
        self.layout_changed = true;
        let mut reveal = if self.change != ViewChange::Stable {
            self.selected_reveal(scroll)
        } else {
            None
        };
        if self.change == ViewChange::Rebuild {
            if let Some(reveal) = &mut reveal {
                reveal.offset = 0.0;
            }
        }
        self.send(Request::Columns(
            self.generation,
            cols,
            self.needed.clone(),
            reveal,
        ))
    }

    pub fn widths(&mut self, widths: &[(usize, f32)]) -> Result<(), String> {
        let Some(folder) = self.folder.as_mut() else {
            return Ok(());
        };
        let folder = Arc::make_mut(folder);
        for &(i, width) in widths {
            if let Some(col) = folder.columns.get_mut(i) {
                col.width = width;
            }
        }
        folder.view.columns = folder.columns.iter().map(|c| (c.key, c.width)).collect();
        self.dirty = true;
        let cols = folder.columns.clone();
        self.layout_changed = true;
        self.send(Request::Layout(cols))
    }

    pub fn fit(&self, indices: Vec<usize>, range: std::ops::Range<usize>) -> Result<(), String> {
        let Some(folder) = &self.folder else {
            return Ok(());
        };
        let indent = 38.0
            + if folder.settings.check_boxes {
                15.0
            } else {
                0.0
            };
        self.send(Request::Fit(
            self.generation,
            folder.columns.clone(),
            range,
            indices,
            indent,
        ))
    }

    pub fn save(&mut self, path: &str) -> Result<(), String> {
        if crate::app::ids::is_settings(path) {
            return Ok(());
        }
        if !self.dirty || crate::app::ids::is_search(path) {
            return Ok(());
        }
        if let Some(folder) = &self.folder {
            let snapshot = if crate::app::ids::same(path, crate::app::ids::GALLERY) {
                shell::viewstate::ViewState {
                    logical_mode: Some(3),
                    icon_size: Some(folder.view.icon_size),
                    ..Default::default()
                }
            } else {
                folder.view.snapshot()
            };
            self.send(Request::Save(path.to_owned(), snapshot))?;
        }
        self.dirty = false;
        Ok(())
    }

    pub fn cancel_search(&mut self) -> bool {
        let busy = self.search.busy();
        self.search.cancel();
        if let Some(folder) = &self.folder {
            let paths = folder
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<HashSet<_>>();
            let selection = &mut self.interaction.selection;
            selection
                .selected
                .retain(|path| paths.contains(path.as_str()));
            for path in [&mut selection.focused, &mut selection.anchor] {
                if path
                    .as_ref()
                    .is_some_and(|path| !paths.contains(path.as_str()))
                {
                    *path = None;
                }
            }
        }
        busy
    }

    pub fn select_created(
        &mut self,
        paths: Vec<String>,
        scroll: &mut super::scroll::State,
    ) -> Result<(), String> {
        let Some(first) = paths.first().cloned() else {
            return Ok(());
        };
        self.pending_selection = Some(crate::app::selection::Selection {
            selected: paths.into_iter().collect(),
            anchor: Some(first.clone()),
            focused: Some(first),
        });
        if !self.apply_created(scroll) {
            self.refresh(scroll)?;
        }
        Ok(())
    }

    fn apply_created(&mut self, scroll: &mut super::scroll::State) -> bool {
        let Some(selection) = &self.pending_selection else {
            return false;
        };
        let Some(folder) = &self.folder else {
            return false;
        };
        let paths = folder
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<HashSet<_>>();
        if !selection
            .selected
            .iter()
            .all(|path| paths.contains(path.as_str()))
        {
            return false;
        }
        self.interaction.selection = self.pending_selection.take().unwrap();
        self.interaction.group_focus = None;
        self.interaction.mark_all();
        if let Some(view) = scroll.view {
            if let Some(row) = self.interaction.focus_row(folder) {
                self.interaction.reveal(row, folder, view, scroll);
            }
        }
        self.layout_changed = true;
        true
    }

    pub fn refresh(&mut self, scroll: &super::scroll::State) -> Result<(), String> {
        self.change = ViewChange::Rebuild;
        let mut reveal = self.selected_reveal(scroll);
        if let Some(reveal) = &mut reveal {
            reveal.offset = 0.0;
        }
        self.generation += 1;
        self.pending.clear();
        self.send(Request::Refresh(
            self.generation,
            0..self.needed.len(),
            reveal,
        ))
    }

    fn relayout(&mut self, scroll: &mut super::scroll::State, change: ViewChange) {
        let Some(folder) = &self.folder else {
            return;
        };
        if change == ViewChange::Rebuild {
            self.cut.clear();
            self.interaction.restore_view(folder);
            scroll.y = 0.0;
        }
        if let Some(previous) = scroll.view {
            let view = super::details::Viewport::new(previous.size, folder, self.navigation_width);
            scroll.layout_folder(view, folder);
            if change != ViewChange::Stable
                && self
                    .interaction
                    .selection
                    .focused
                    .as_ref()
                    .is_some_and(|path| self.interaction.selection.selected.contains(path))
            {
                if let Some(row) = self.interaction.focus_row(folder) {
                    self.interaction.reveal(row, folder, view, scroll);
                }
            }
            self.needed = view.needed(folder, scroll.y);
        }
        self.layout_changed = true;
        self.rows.clear();
        self.pending.clear();
        self.interaction.mark_all();
    }

    pub fn receive(
        &mut self,
        scroll: &mut super::scroll::State,
    ) -> Result<Option<std::ops::Range<usize>>, String> {
        let mut changed: Option<std::ops::Range<usize>> = None;
        let mut events = Vec::new();
        while let Ok(update) = self.updates.try_recv() {
            let update = match update {
                Update::SearchBatch(epoch, folder, counts) if self.search.busy() => {
                    Update::Changed(epoch, folder, Vec::new(), counts)
                }
                Update::SearchBatch(..) => continue,
                update => update,
            };
            let rows = match update {
                Update::FilterMenu(token, key, labels) => {
                    self.filter_menu = Some((token, key, labels));
                    continue;
                }
                Update::Pane(token, model) => {
                    self.pane_model = Some((token, model));
                    changed = Some(0..0);
                    continue;
                }
                Update::Grid(epoch, model) if epoch == self.generation => {
                    let Some(folder) = self.folder.as_mut() else {
                        continue;
                    };
                    if self.grid_request.is_none_or(|key| {
                        key.0 != folder.view.mode
                            || key.1 != folder.view.icon_size
                            || key.2 != model.size.0.to_bits()
                            || key.3 != model.size.1.to_bits()
                            || key.4 != model.layout_id
                    }) {
                        continue;
                    }
                    let first = folder.grid.as_ref().is_none_or(|old| {
                        old.kind != model.kind
                            || old.spec.kind != model.spec.kind
                            || old.spec.label_indent != model.spec.label_indent
                            || old.spec.column_major != model.spec.column_major
                            || old.kind != super::grid::Kind::Gallery
                                && old.spec.icon != model.spec.icon
                    });
                    let redraw = match (&folder.grid, scroll.view) {
                        (Some(old), Some(view)) => !old.same_visible(
                            &model,
                            (scroll.x, scroll.y),
                            (view.right, view.rows_height),
                        ),
                        _ => true,
                    };
                    Arc::make_mut(folder).grid = Some(model);
                    self.layout_changed = true;
                    if redraw {
                        self.interaction.mark_all();
                    }
                    if first {
                        self.pending.clear();
                        self.rows.clear();
                    }
                    if redraw {
                        changed = Some(0..folder.order.len());
                    } else if changed.is_none() {
                        changed = Some(0..0);
                    }
                    continue;
                }
                Update::WatchError(error) => {
                    eprintln!("native folder watcher: {error}");
                    continue;
                }
                Update::Event(event) => {
                    if matches!(
                        event.kind,
                        notify::EventKind::Modify(notify::event::ModifyKind::Name(
                            notify::event::RenameMode::Both
                        ))
                    ) && event.paths.len() == 2
                    {
                        let from = event.paths[0].to_string_lossy();
                        let to = event.paths[1].to_string_lossy().into_owned();
                        let selection = &mut self.interaction.selection;
                        if let Some(old) = selection
                            .selected
                            .iter()
                            .find(|path| crate::app::ids::same(path, &from))
                            .cloned()
                        {
                            selection.selected.remove(&old);
                            selection.selected.insert(to.clone());
                        }
                        for path in [&mut selection.focused, &mut selection.anchor] {
                            if path
                                .as_ref()
                                .is_some_and(|path| crate::app::ids::same(path, &from))
                            {
                                *path = Some(to.clone());
                            }
                        }
                    }
                    events.push(event);
                    continue;
                }
                Update::Changed(epoch, folder, rows, counts) => {
                    let searching = folder
                        .crumbs
                        .last()
                        .is_some_and(|crumb| crate::app::ids::is_search(&crumb.id));
                    let mut retained = if searching {
                        self.rows
                            .drain()
                            .filter_map(|(row, ready)| {
                                let old = self.folder.as_ref()?;
                                let entry = old.entries.get(*old.order.get(row)?)?;
                                Some((entry.path.clone(), ready))
                            })
                            .collect::<HashMap<_, _>>()
                    } else {
                        HashMap::new()
                    };
                    self.enumerations = counts.0;
                    self.entry_reads = counts.1;
                    let previous_focus = self
                        .folder
                        .as_ref()
                        .and_then(|folder| self.interaction.focus_row(folder));
                    let folder = if let Some(current) = self.folder.as_mut() {
                        let current = Arc::make_mut(current);
                        current.entries = folder.entries.clone();
                        current.order = folder.order.clone();
                        current.headers = folder.headers.clone();
                        current.home_filter = folder.home_filter;
                        current.filter = folder.filter.clone();
                        current.library_origins = folder.library_origins.clone();
                        current.network_notice = folder.network_notice.clone();
                        current.rebuild_layout();
                        Arc::new(current.clone())
                    } else {
                        folder
                    };
                    self.folder = Some(folder.clone());
                    let paths = folder
                        .entries
                        .iter()
                        .map(|entry| entry.path.as_str())
                        .collect::<HashSet<_>>();
                    if !self.search.busy() {
                        self.interaction
                            .selection
                            .selected
                            .retain(|path| paths.contains(path.as_str()));
                        for path in [
                            &mut self.interaction.selection.focused,
                            &mut self.interaction.selection.anchor,
                        ] {
                            if path
                                .as_ref()
                                .is_some_and(|path| !paths.contains(path.as_str()))
                            {
                                *path = None;
                            }
                        }
                    }
                    if self.interaction.selection.focused.is_none() {
                        if let Some(row) = previous_focus
                            .or_else(|| {
                                (searching
                                    || folder
                                        .crumbs
                                        .last()
                                        .is_some_and(|crumb| discovery(&crumb.id)))
                                .then_some(0)
                            })
                            .filter(|_| !folder.order.is_empty())
                        {
                            self.interaction.selection.focused = Some(
                                folder.entries[folder.order[row.min(folder.order.len() - 1)]]
                                    .path
                                    .clone(),
                            );
                            if self.interaction.selection.anchor.is_none() {
                                self.interaction.selection.anchor =
                                    self.interaction.selection.focused.clone();
                            }
                        }
                    }
                    if self.interaction.group_focus.as_ref().is_some_and(|label| {
                        !folder
                            .layout
                            .headers
                            .iter()
                            .any(|header| header.label == *label)
                    }) {
                        self.interaction.group_focus = None;
                    }
                    let change = if epoch == self.generation {
                        std::mem::replace(&mut self.change, ViewChange::Stable)
                    } else {
                        ViewChange::Stable
                    };
                    self.relayout(scroll, change);
                    if searching {
                        for (index, &entry) in folder.order.iter().enumerate() {
                            if let Some(mut row) = retained.remove(&folder.entries[entry].path) {
                                row.index = index;
                                self.rows.insert(index, row);
                            }
                        }
                    }
                    self.apply_created(scroll);
                    changed = Some(0..folder.order.len());
                    if epoch == self.generation {
                        rows
                    } else {
                        Vec::new()
                    }
                }
                Update::Loaded(folder) => {
                    self.folder = Some(folder);
                    changed = Some(0..usize::MAX);
                    continue;
                }
                Update::Rows(epoch, rows) if epoch == self.generation => rows,
                Update::Order(epoch, order, headers, rows) if epoch == self.generation => {
                    if let Some(folder) = self.folder.as_mut() {
                        let folder = Arc::make_mut(folder);
                        folder.order = order;
                        folder.headers = headers;
                        folder.rebuild_layout();
                    }
                    let change = std::mem::replace(&mut self.change, ViewChange::Stable);
                    self.relayout(scroll, change);
                    changed = Some(0..self.folder.as_ref().map_or(0, |folder| folder.order.len()));
                    rows
                }
                Update::Fit(epoch, widths) if epoch == self.generation => {
                    self.widths(&widths)?;
                    changed = Some(0..self.folder.as_ref().map_or(0, |f| f.order.len()));
                    continue;
                }
                Update::SearchFinished(result) => {
                    self.cancel_search();
                    if std::env::var("SW_SEARCH_PROBE").as_deref() == Ok("1") {
                        if let Some(folder) = &self.folder {
                            let rows = folder
                                .order
                                .iter()
                                .map(|&index| {
                                    let entry = &folder.entries[index];
                                    serde_json::json!({"name": entry.name, "path": entry.path})
                                })
                                .collect::<Vec<_>>();
                            eprintln!(
                                "native search snapshot {}",
                                serde_json::json!({"path": folder.crumbs.last().map(|crumb| &crumb.id), "rows": rows, "workers": self.search.workers()})
                            );
                        }
                    }
                    if let Err(error) = result {
                        eprintln!("native search: {error}");
                    }
                    changed = Some(0..self.folder.as_ref().map_or(0, |folder| folder.order.len()));
                    continue;
                }
                Update::Error(error) => return Err(error),
                _ => continue,
            };
            for row in rows {
                let index = row.index;
                self.pending.remove(&index);
                if !self.needed.contains(&index) {
                    continue;
                }
                self.rows.insert(index, row);
                changed = Some(match changed {
                    Some(old) => old.start.min(index)..old.end.max(index + 1),
                    None => index..index + 1,
                });
            }
        }
        if !events.is_empty() {
            self.generation += 1;
            self.pending.clear();
            self.send(Request::Change(
                self.generation,
                events,
                self.needed.clone(),
            ))?;
        }
        Ok(changed)
    }

    pub fn request_pane(
        &self,
        token: u64,
        kind: crate::app::pane_model::Kind,
        selection: crate::app::pane_model::Selection,
        width: u32,
    ) -> Result<(), String> {
        self.send(Request::Pane(token, kind, selection, width))
    }

    pub fn request(&mut self, visible: std::ops::Range<usize>) -> Result<(), String> {
        if self.needed != visible {
            self.needed = visible.clone();
            self.send(Request::Viewport(visible.clone()))?;
            self.pending.clear();
            self.rows.retain(|index, _| visible.contains(index));
        }
        let missing = visible
            .filter(|i| {
                self.folder
                    .as_ref()
                    .is_some_and(|folder| !folder.layout.folded[*i])
                    && !self.rows.contains_key(i)
                    && self.pending.insert(*i)
            })
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            self.send(Request::Rows(self.generation, missing))?;
        }
        Ok(())
    }
}

fn reveal_range(
    folder: &Folder,
    range: std::ops::Range<usize>,
    reveal: Option<Reveal>,
) -> std::ops::Range<usize> {
    reveal
        .and_then(|reveal| {
            folder
                .order
                .iter()
                .position(|&i| folder.entries[i].path == reveal.path)
                .map(|row| {
                    let view = super::details::Viewport::at(
                        reveal.view.size,
                        folder,
                        reveal.view.x,
                        reveal.view.y,
                    );
                    let offset = crate::app::selection::reveal_offset(
                        reveal.offset,
                        view.rows_height,
                        view.row_top(folder, row),
                        view.row_bottom(folder, row),
                    )
                    .clamp(0.0, view.maximum_y())
                    .round();
                    view.needed(folder, offset)
                })
        })
        .unwrap_or(range)
}

fn read_cells(
    folder: &Folder,
    entry: &ShellEntry,
    cols: &[columns::Col],
    keys: &[windows::Win32::Foundation::PROPERTYKEY],
    items: &shell::columns::Items,
) -> Vec<String> {
    let values = if keys.is_empty() {
        Vec::new()
    } else {
        keys.iter()
            .copied()
            .zip(shell::columns::texts_with_id(
                items,
                &entry.path,
                keys,
                &entry.absolute,
            ))
            .collect::<Vec<_>>()
    };
    cols.iter()
        .map(|c| {
            if c.key == windows::Win32::Storage::EnhancedStorage::PKEY_Size
                && folder.crumbs.last().is_some_and(|crumb| {
                    crate::app::ids::same(&crumb.id, crate::app::ids::RECYCLE_BIN)
                })
            {
                crate::app::format::size_kb(entry.size)
            } else {
                columns::cell_text(entry, &c.key, Some(&values))
            }
        })
        .collect()
}

fn ready_rows(
    folder: &Folder,
    indices: Vec<usize>,
    items: &shell::columns::Items,
    icons: &mut RowIcons,
) -> Vec<Row> {
    let keys = folder
        .columns
        .iter()
        .map(|column| column.key)
        .filter(columns::needs_property)
        .collect::<Vec<_>>();
    indices
        .into_iter()
        .filter_map(|index| {
            let entry = &folder.entries[*folder.order.get(index)?];
            let grid = folder.grid.is_some();
            let (icon, image) = if grid {
                (entry.icon, None)
            } else {
                row_icon(entry, icons)
            };
            Some(Row {
                index,
                cells: if grid {
                    Vec::new()
                } else {
                    read_cells(folder, entry, &folder.columns, &keys, items)
                },
                icon,
                image,
                picture: folder
                    .grid
                    .as_ref()
                    .map(|_| super::grid::picture(folder, index)),
            })
        })
        .collect()
}

#[derive(Default)]
struct RowIcons {
    images: HashMap<u32, Arc<Rgba>>,
    resolved: HashMap<String, u32>,
    parent: Option<(String, IShellFolder)>,
}

impl RowIcons {
    fn clear(&mut self) {
        self.images.clear();
        self.resolved.clear();
        self.parent = None;
    }

    fn pending_icon(&mut self, path: &str) -> u32 {
        if let Some(&icon) = self.resolved.get(path) {
            return icon;
        }
        let icon = self.icon_in_parent(path).unwrap_or_else(|| shell::icons::file_icon(path));
        self.resolved.insert(path.to_owned(), icon);
        icon
    }

    fn icon_in_parent(&mut self, path: &str) -> Option<u32> {
        let file = std::path::Path::new(path);
        let parent = file.parent()?.to_str()?;
        let name = file.file_name()?.to_str()?;
        if self.parent.as_ref().is_none_or(|(bound, _)| bound != parent) {
            shell::com_init();
            let folder = shell::Pidl::parse(parent).ok()?.folder().ok()?;
            self.parent = Some((parent.to_owned(), folder));
        }
        let (_, folder) = self.parent.as_ref()?;
        shell::icons::icon_in(folder, name)
    }
}

fn row_icon(entry: &ShellEntry, icons: &mut RowIcons) -> (u32, Option<Arc<Rgba>>) {
    if icons.images.len() >= 256 {
        icons.images.clear();
    }
    let path = &entry.path;
    let icon = if entry.icon_pending {
        icons.pending_icon(path)
    } else {
        entry.icon
    };
    let images = &mut icons.images;
    if let Some(image) = images.get(&icon) {
        return (icon, Some(image.clone()));
    }
    let image = match super::icons::small(icon) {
        Ok(image) => image,
        Err(error) => {
            eprintln!("native row icon {path}: {error}; using generic icon");
            let generic = if entry.is_dir {
                shell::icons::folder_icon()
            } else {
                shell::icons::generic_icon(path)
            };
            if let Some(image) = images.get(&generic) {
                return (generic, Some(image.clone()));
            }
            let image = match super::icons::small(generic) {
                Ok(image) => image,
                Err(error) => {
                    eprintln!("native generic icon {path}: {error}; leaving this icon empty");
                    return (generic, None);
                }
            };
            images.insert(generic, image.clone());
            return (generic, Some(image));
        }
    };
    images.insert(icon, image.clone());
    (icon, Some(image))
}

#[cfg(test)]
mod tests {
    fn list_for_folder(
        folder: super::Folder,
        updates: std::sync::mpsc::Receiver<super::Update>,
        requests: std::sync::mpsc::Sender<super::Request>,
    ) -> super::List {
        super::List {
            _alive: std::sync::Arc::new(()),
            navigation_width: 202.0,
            header: Default::default(),
            interaction: Default::default(),
            folder: Some(std::sync::Arc::new(folder)),
            rows: Default::default(),
            cut: Default::default(),
            search: Default::default(),
            pane_model: None,
            enumerations: 1,
            entry_reads: 0,
            dirty: false,
            layout_changed: false,
            pending_selection: None,
            filter_menu: None,
            requested_filter: None,
            pending: Default::default(),
            updates,
            requests,
            needed: 0..0,
            generation: 0,
            change: super::ViewChange::Stable,
            grid_request: None,
        }
    }

    #[test]
    fn large_folder_draws_visible_names_without_worker_rows_or_grid_labels() {
        use super::super::{details, grid, render, text, theme};
        use super::*;
        let mut folder = load(concat!(env!("CARGO_MANIFEST_DIR"), "\\src\\ui"), None).unwrap();
        let entry = folder.entries[0].clone();
        folder.entries = Arc::new(
            (0..5019)
                .map(|index| {
                    let mut entry = entry.clone();
                    entry.name = format!("Visible item {index:05}");
                    entry.path = format!("C:\\Visible item {index:05}");
                    entry
                })
                .collect(),
        );
        folder.order = Arc::new((0..5019).collect());
        folder.headers = Arc::new(Vec::new());
        folder.collapsed.clear();
        folder.view.group = None;
        folder.settings.compact = true;
        folder.settings.check_boxes = false;
        folder.settings.single_click = false;
        folder.columns = columns::shown_columns(vec![(
            windows::Win32::Storage::EnhancedStorage::PKEY_ItemNameDisplay,
            300.0,
        )]);
        folder.rebuild_layout();
        let (_sender, updates) = mpsc::channel();
        let (requests, _receiver) = mpsc::channel();
        let mut list = list_for_folder(folder, updates, requests);
        let renderer = render::Renderer::new().unwrap();
        let mut text = text::TextEngine::new(96).unwrap();
        let mut theme = theme::Theme::system();
        theme.content = windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F {
            r: 25.0 / 255.0,
            g: 25.0 / 255.0,
            b: 25.0 / 255.0,
            a: 1.0,
        };
        theme.dui_text = windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };
        for (mode, size) in [
            (6, 16),
            (1, 256),
            (2, 96),
            (3, 48),
            (4, 16),
            (5, 16),
            (7, 48),
            (8, 32),
        ] {
            let folder = Arc::make_mut(list.folder.as_mut().unwrap());
            folder.view.mode = mode;
            folder.view.icon_size = size;
            folder.grid = grid::Model::new(folder, (596.0, 559.0)).map(|mut model| {
                model.labels.fill(None);
                Arc::new(model)
            });
            let view = details::Viewport::new((1100.0, 719.0), folder, 202.0);
            for end in [false, true] {
                let x = if end { view.maximum_x() } else { 0.0 };
                let y = if end { view.maximum_y() } else { 0.0 };
                let folder = list.folder.as_ref().unwrap();
                let pixels = render::test_pixels(
                    &renderer.canvas,
                    (view.right as u32, view.rows_height as u32),
                    || {
                        let mut original = Default::default();
                        unsafe {
                            renderer.canvas.context.GetTransform(&mut original);
                        }
                        let mut transform = original;
                        transform.M31 = -x;
                        transform.M32 = -y;
                        unsafe {
                            renderer.canvas.context.SetTransform(&transform);
                        }
                        let result = if folder.grid.is_some() {
                            grid::draw(
                                &list,
                                &mut Default::default(),
                                &renderer.canvas,
                                &mut text,
                                &theme,
                                view,
                                windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                                    left: x,
                                    top: y,
                                    right: x + view.right,
                                    bottom: y + view.rows_height,
                                },
                            )
                        } else {
                            details::draw_rows(
                                &list,
                                &mut Default::default(),
                                &renderer.canvas,
                                &mut text,
                                &theme,
                                view,
                                folder.layout.visible(y, view.rows_height, view.plate),
                            )
                        };
                        unsafe {
                            renderer.canvas.context.SetTransform(&original);
                        }
                        result
                    },
                )
                .unwrap();
                assert!(
                    pixels
                        .chunks_exact(3)
                        .filter(|pixel| pixel.iter().all(|&channel| channel > 95))
                        .count()
                        > 100,
                    "mode {mode}, end {end} rendered no visible names"
                );
            }
        }
    }

    #[test]
    fn column_filter_uses_all_entries_and_clears_a_resumed_filtered_view() {
        use super::{sort, List, Worker, HWND};
        let root = std::env::temp_dir().join(format!("sw-native-filter19-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let files = [
            root.join("alpha.txt"),
            root.join("beta.txt"),
            root.join("zeta.bin"),
        ];
        struct Fixture(std::path::PathBuf, [std::path::PathBuf; 3]);
        impl Drop for Fixture {
            fn drop(&mut self) {
                for file in &self.1 {
                    std::fs::remove_file(file).unwrap();
                }
                std::fs::remove_dir(&self.0).unwrap();
            }
        }
        for file in &files {
            std::fs::write(file, b"owned filter fixture").unwrap();
        }
        let _fixture = Fixture(root.clone(), files);
        let hwnd = unsafe {
            windows::Win32::UI::WindowsAndMessaging::CreateWindowExW(
                Default::default(),
                windows::core::w!("STATIC"),
                windows::core::w!("NativeFilter19Test"),
                Default::default(),
                0,
                0,
                1,
                1,
                Some(windows::Win32::UI::WindowsAndMessaging::HWND_MESSAGE),
                None,
                None,
                None,
            )
            .unwrap()
        };
        struct Window(HWND);
        impl Drop for Window {
            fn drop(&mut self) {
                unsafe {
                    windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0).unwrap();
                }
            }
        }
        let _window = Window(hwnd);
        let worker = Worker::start().unwrap();
        let mut list =
            List::start(&worker, hwnd, root.to_string_lossy().into_owned(), None).unwrap();
        let mut scroll = super::super::scroll::State::default();
        let wait = |list: &mut List, scroll: &mut super::super::scroll::State, count| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                list.receive(scroll).unwrap();
                if list
                    .folder
                    .as_ref()
                    .is_some_and(|folder| folder.order.len() == count)
                {
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        wait(&mut list, &mut scroll, 3);
        list.filter_menu(42, sort::SortKey::Type).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while list.filter_menu.is_none() {
            list.receive(&mut scroll).unwrap();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(list.filter_menu.as_ref().unwrap().2.len(), 2);
        let folder = list.folder.as_ref().unwrap();
        let label = folder
            .entries
            .iter()
            .find(|entry| entry.path.ends_with("alpha.txt"))
            .unwrap()
            .type_name
            .clone();
        list.set_filter(Some(crate::app::filter::Filter {
            key: sort::SortKey::Type,
            labels: vec![label],
        }))
        .unwrap();
        wait(&mut list, &mut scroll, 2);
        let cached = list.folder.clone().unwrap();
        drop(list);
        let mut resumed =
            List::resume(&worker, hwnd, root.to_string_lossy().into_owned(), cached).unwrap();
        wait(&mut resumed, &mut scroll, 2);
        resumed.refresh(&scroll).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while resumed.receive(&mut scroll).unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(resumed.folder.as_ref().unwrap().order.len(), 2);
        assert!(resumed.folder.as_ref().unwrap().filter.is_some());
        resumed.set_filter(None).unwrap();
        wait(&mut resumed, &mut scroll, 3);
        assert!(resumed.folder.as_ref().unwrap().filter.is_none());
        drop(resumed);
        drop(worker);
    }

    #[test]
    fn cancelled_search_discards_a_batch_already_queued_for_the_ui() {
        use super::*;
        let root = std::env::temp_dir().join(format!("sw-search18-queued-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("owned.txt"), b"owned queued search dummy").unwrap();
        let original = load(root.to_str().unwrap(), None).unwrap();
        let mut expanded = original.clone();
        let mut entry = expanded.entries[0].clone();
        entry.path.push_str(".queued");
        Arc::make_mut(&mut expanded.entries).push(entry);
        expanded.order = Arc::new(vec![0, 1]);
        expanded.rebuild_layout();
        let (sender, updates) = mpsc::channel();
        let (requests, _receiver) = mpsc::channel();
        let mut list = list_for_folder(original, updates, requests);
        list.needed = 0..8;
        let mut scroll = super::super::scroll::State::default();
        sender
            .send(Update::SearchBatch(0, Arc::new(expanded.clone()), (1, 0)))
            .unwrap();
        list.receive(&mut scroll).unwrap();
        assert_eq!(list.folder.as_ref().unwrap().entries.len(), 1);
        list.search
            .current
            .store(true, std::sync::atomic::Ordering::Release);
        sender
            .send(Update::SearchBatch(0, Arc::new(expanded), (1, 0)))
            .unwrap();
        list.receive(&mut scroll).unwrap();
        assert_eq!(list.folder.as_ref().unwrap().entries.len(), 2);
        drop(list);
        assert_eq!(
            std::fs::read(root.join("owned.txt")).unwrap(),
            b"owned queued search dummy"
        );
        std::fs::remove_file(root.join("owned.txt")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn queued_navigation_drains_each_folders_save_and_stops_on_close() {
        use super::*;
        use windows::core::w;
        use windows::Win32::UI::WindowsAndMessaging::*;
        let root = std::env::temp_dir().join(format!("sw-native-worker-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let paths = [root.join("first"), root.join("second")];
        for path in &paths {
            std::fs::create_dir(path).unwrap();
        }
        let hwnd = unsafe {
            CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                w!(""),
                Default::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .unwrap()
        };
        let worker = Worker::start().unwrap();
        let mut current: Option<List> = None;
        for (index, path) in paths.iter().enumerate() {
            let next = List::start(&worker, hwnd, path.to_str().unwrap().into(), None).unwrap();
            drop(current.take());
            let mut next = next;
            loop {
                match next
                    .updates
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap()
                {
                    Update::Loaded(folder) => {
                        next.folder = Some(folder);
                        break;
                    }
                    Update::Error(error) => panic!("{error}"),
                    _ => {}
                }
            }
            let folder = Arc::make_mut(next.folder.as_mut().unwrap());
            folder.view.columns[0].1 = 301.0 + index as f32;
            next.dirty = true;
            next.save(path.to_str().unwrap()).unwrap();
            current = Some(next);
        }
        drop(current);
        drop(worker);
        unsafe {
            DestroyWindow(hwnd).unwrap();
        }
        for (index, path) in paths.iter().enumerate() {
            let saved = shell::viewstate::read(path.to_str().unwrap());
            assert_eq!(saved.columns[0].1, 301 + index as u32);
            std::fs::remove_dir(path).unwrap();
        }
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn pasted_selection_waits_for_every_created_item_and_then_updates_together() {
        use super::*;
        let root =
            std::env::temp_dir().join(format!("sw-native-paste-selection-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let initial = load(root.to_str().unwrap(), None).unwrap();
        let (sender, updates) = mpsc::channel();
        let (requests, _receiver) = mpsc::channel();
        let mut list = List {
            navigation_width: 202.0,
            _alive: Arc::new(()),
            header: Default::default(),
            interaction: Default::default(),
            folder: Some(Arc::new(initial)),
            rows: HashMap::new(),
            cut: HashSet::new(),
            search: Default::default(),
            pane_model: None,
            enumerations: 1,
            entry_reads: 0,
            dirty: false,
            layout_changed: false,
            pending_selection: None,
            filter_menu: None,
            requested_filter: None,
            pending: HashSet::new(),
            updates,
            requests,
            needed: 0..8,
            generation: 0,
            change: ViewChange::Stable,
            grid_request: None,
        };
        let mut scroll = super::super::scroll::State::default();
        let paths = [root.join("alpha.txt"), root.join("beta.txt")]
            .map(|path| path.to_string_lossy().into_owned());
        list.select_created(paths.to_vec(), &mut scroll).unwrap();
        assert!(list.interaction.selection.selected.is_empty());
        std::fs::write(&paths[0], b"a").unwrap();
        let partial = load(root.to_str().unwrap(), None).unwrap();
        sender
            .send(Update::Changed(1, Arc::new(partial), Vec::new(), (2, 0)))
            .unwrap();
        list.receive(&mut scroll).unwrap();
        assert!(list.interaction.selection.selected.is_empty());
        assert!(list.pending_selection.is_some());
        std::fs::write(&paths[1], b"b").unwrap();
        let complete = load(root.to_str().unwrap(), None).unwrap();
        sender
            .send(Update::Changed(1, Arc::new(complete), Vec::new(), (3, 0)))
            .unwrap();
        list.receive(&mut scroll).unwrap();
        assert_eq!(
            list.interaction.selection.selected,
            paths.iter().cloned().collect()
        );
        assert_eq!(
            list.interaction.selection.focused.as_deref(),
            Some(paths[0].as_str())
        );
        assert!(list.pending_selection.is_none());
        list.select_created(vec![paths[1].clone()], &mut scroll)
            .unwrap();
        assert_eq!(
            list.interaction.selection.selected,
            [paths[1].clone()].into_iter().collect()
        );
        assert!(list.pending_selection.is_none());
        drop(list);
        for path in paths {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn grouped_home_and_end_select_items_while_up_can_select_a_header() {
        use super::*;
        let root =
            std::env::temp_dir().join(format!("sw-native-group-keys-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("alpha.txt"), b"a").unwrap();
        std::fs::write(root.join("beta.txt"), b"b").unwrap();
        let mut folder = load(root.to_str().unwrap(), None).unwrap();
        folder.headers = Arc::new(vec!["A".into(), "B".into()]);
        folder.collapsed.clear();
        folder.rebuild_layout();
        let mut state = super::super::interaction::State::default();
        state.has_focus = true;
        let mut scroll = super::super::scroll::State::default();
        let view = super::super::details::Viewport::new((1102.0, 220.0), &folder, 202.0);
        scroll.layout(view, &folder.layout);
        scroll.y = view.maximum_y();
        let mods = crate::app::selection::Modifiers::default();
        let _ = state.key(0x24, &folder, &mut scroll, mods);
        assert!(state.group_focus.is_none());
        assert_eq!(state.focus_row(&folder), Some(0));
        assert_eq!(state.selection.selected.len(), 1);
        assert_eq!(scroll.y, 0.0);
        let _ = state.key(0x26, &folder, &mut scroll, mods);
        assert_eq!(state.group_focus.as_deref(), Some("A"));
        let _ = state.key(0x23, &folder, &mut scroll, mods);
        assert!(state.group_focus.is_none());
        assert_eq!(state.focus_row(&folder), Some(1));
        assert_eq!(state.selection.selected.len(), 1);
        assert_eq!(scroll.y, view.maximum_y());
        std::fs::remove_file(root.join("alpha.txt")).unwrap();
        std::fs::remove_file(root.join("beta.txt")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn collapsing_icon_groups_retains_labels_and_never_enters_details_rendering() {
        use super::*;
        let root =
            std::env::temp_dir().join(format!("sw-native-grid-collapse-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("alpha.txt"), b"a").unwrap();
        std::fs::write(root.join("beta.txt"), b"b").unwrap();
        let mut folder = load(root.to_str().unwrap(), None).unwrap();
        folder.view.mode = 3;
        folder.view.icon_size = 48;
        folder.headers = Arc::new(vec!["A".into(), "B".into()]);
        folder.collapsed.clear();
        folder.rebuild_layout();
        assert!(folder.awaiting_grid());
        folder.grid = Some(Arc::new(
            super::super::grid::Model::new(&folder, (898.0, 559.0)).unwrap(),
        ));
        let old_model = folder.grid.clone().unwrap();
        let old_layout = Arc::downgrade(&folder.layout);
        let label = folder.grid.as_ref().unwrap().labels[1].clone().unwrap();
        folder.set_collapsed(["A".to_owned()].into_iter().collect());
        assert!(!folder.awaiting_grid());
        let model = folder.grid.as_ref().unwrap();
        assert_eq!(model.layout.cells[0][3], 0.0);
        assert!(model.layout.cells[1][3] > 0.0);
        assert!(Arc::ptr_eq(model.labels[1].as_ref().unwrap(), &label));
        assert_eq!(model.layout_id, Arc::as_ptr(&folder.layout) as usize);
        assert!(old_layout.upgrade().is_some());
        drop(old_model);
        assert!(old_layout.upgrade().is_none());
        folder.set_collapsed(HashSet::new());
        assert!(folder.grid.as_ref().unwrap().layout.cells[0][3] > 0.0);
        std::fs::remove_file(root.join("alpha.txt")).unwrap();
        std::fs::remove_file(root.join("beta.txt")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn icon_typeahead_aligns_an_offscreen_match_at_the_top() {
        use super::*;
        let root =
            std::env::temp_dir().join(format!("sw-native-grid-typeahead-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        for name in ["alpha.txt", "beta.txt", "gamma.txt"] {
            std::fs::write(root.join(name), b"a").unwrap();
        }
        let mut folder = load(root.to_str().unwrap(), None).unwrap();
        folder.panes = (false, false);
        folder.settings.status_bar = true;
        folder.view.mode = 2;
        folder.view.icon_size = 96;
        folder.grid = Some(Arc::new(
            super::super::grid::Model::new(&folder, (120.0, 180.0)).unwrap(),
        ));
        let view = super::super::details::Viewport::new((322.0, 340.0), &folder, 202.0);
        let mut scroll = super::super::scroll::State::default();
        scroll.layout_folder(view, &folder);
        let mut state = super::super::interaction::State::default();
        state.has_focus = true;
        state.character('b', &folder, &mut scroll);
        assert_eq!(state.focus_row(&folder), Some(1));
        assert_eq!(scroll.y, 123.0);
        for name in ["alpha.txt", "beta.txt", "gamma.txt"] {
            std::fs::remove_file(root.join(name)).unwrap();
        }
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn live_data_survives_a_newer_sort_without_accepting_stale_cells() {
        use super::*;
        let root = std::env::temp_dir().join(format!("sw-native-live-sort-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("alpha.txt"), b"a").unwrap();
        let mut initial = load(root.to_str().unwrap(), None).unwrap();
        initial.columns[0].width = 300.0;
        std::fs::write(root.join("beta.txt"), b"b").unwrap();
        let updated = load(root.to_str().unwrap(), None).unwrap();
        let (sender, updates) = mpsc::channel();
        let (requests, _receiver) = mpsc::channel();
        let mut list = List {
            navigation_width: 202.0,
            _alive: Arc::new(()),
            header: Default::default(),
            interaction: Default::default(),
            folder: Some(Arc::new(initial)),
            rows: HashMap::new(),
            cut: HashSet::new(),
            search: Default::default(),
            pane_model: None,
            enumerations: 1,
            entry_reads: 0,
            dirty: true,
            layout_changed: false,
            pending_selection: None,
            filter_menu: None,
            requested_filter: None,
            pending: HashSet::new(),
            updates,
            requests,
            needed: 0..8,
            generation: 2,
            change: ViewChange::Sort,
            grid_request: None,
        };
        let mut scroll = super::super::scroll::State::default();
        sender
            .send(Update::Changed(
                1,
                Arc::new(updated),
                vec![Row {
                    index: 1,
                    picture: None,
                    cells: vec!["stale".into()],
                    icon: 0,
                    image: None,
                }],
                (1, 1),
            ))
            .unwrap();
        list.receive(&mut scroll).unwrap();
        assert_eq!(list.folder.as_ref().unwrap().entries.len(), 2);
        assert_eq!(list.folder.as_ref().unwrap().columns[0].width, 300.0);
        assert!(list.rows.is_empty());
        assert!(list.change == ViewChange::Sort);
        sender
            .send(Update::Order(
                2,
                Arc::new(vec![1, 0]),
                Arc::new(Vec::new()),
                Vec::new(),
            ))
            .unwrap();
        list.receive(&mut scroll).unwrap();
        let folder = list.folder.as_ref().unwrap();
        assert_eq!(folder.order.len(), 2);
        assert!(folder
            .order
            .iter()
            .all(|&index| index < folder.entries.len()));
        assert!(list.change == ViewChange::Stable);
        drop(list);
        std::fs::remove_file(root.join("alpha.txt")).unwrap();
        std::fs::remove_file(root.join("beta.txt")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn shortcut_overlay_survives_deferred_identity_lookup() {
        use windows::core::{Interface, PCWSTR};
        use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        crate::shell::com_init();
        let root = std::env::temp_dir().join(format!("sw-native-overlay-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let target = root.join("target.txt");
        let shortcut = root.join("shortcut.lnk");
        std::fs::write(&target, b"dummy").unwrap();
        unsafe {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            link.SetPath(PCWSTR(
                crate::shell::wide(target.to_str().unwrap()).as_ptr(),
            ))
            .unwrap();
            link.cast::<IPersistFile>()
                .unwrap()
                .Save(
                    PCWSTR(crate::shell::wide(shortcut.to_str().unwrap()).as_ptr()),
                    true,
                )
                .unwrap();
        }
        let entries = crate::shell::listing::list(root.to_str().unwrap()).unwrap();
        let entry = entries.iter().find(|entry| entry.is_link).unwrap();
        assert!(entry.icon_pending);
        let (shown, _) = super::row_icon(entry, &mut super::RowIcons::default());
        let resolved = crate::shell::icons::file_icon(shortcut.to_str().unwrap());
        assert!(resolved >> 24 > 0);
        assert_eq!(resolved >> 24, shown >> 24);
        assert_eq!(
            crate::shell::icons::icon(resolved, 16).unwrap().pixels,
            crate::shell::icons::icon(shown, 16).unwrap().pixels
        );
        let mut settings = crate::shell::listing::settings();
        let mut styled = entries.iter().find(|entry| !entry.is_link).unwrap().clone();
        for (attributes, expected) in [(0, 0), (0x800, 1), (0x4000, 2), (0x4800, 2)] {
            styled.file_attributes = attributes;
            settings.compressed_color = true;
            assert_eq!(crate::app::format::name_style(&styled, &settings), expected);
            settings.compressed_color = false;
            assert_eq!(crate::app::format::name_style(&styled, &settings), 0);
        }
        std::fs::remove_file(shortcut).unwrap();
        std::fs::remove_file(target).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn immediate_and_deferred_elevated_icons_match_explorer() {
        use super::*;
        let windows = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let path = format!(r"{windows}\System32");
        let mut entry = shell::listing::list(&path)
            .unwrap()
            .into_iter()
            .find(|e| e.name.eq_ignore_ascii_case("ComputerDefaults.exe"))
            .unwrap();
        let resolved = shell::icons::file_icon(&entry.path);
        entry.icon_pending = false;
        entry.icon = resolved;
        let (_, immediate) = row_icon(&entry, &mut RowIcons::default());
        let immediate = immediate.unwrap();
        entry.icon_pending = true;
        entry.icon = shell::icons::generic_icon(&entry.path);
        let (_, deferred) = row_icon(&entry, &mut RowIcons::default());
        let deferred = deferred.unwrap();
        assert_eq!((immediate.width, immediate.height), (16, 16));
        assert_eq!((deferred.width, deferred.height), (16, 16));
        assert_eq!(immediate.pixels.len(), 16 * 16 * 4);
        assert_eq!(immediate.pixels, deferred.pixels);
        use windows::core::Interface;
        use windows::Win32::UI::Shell::IShellIcon;
        const GIL_FORSHELL: u32 = 0x0002;
        const GIL_CHECKSHIELD: u32 = 0x0200;
        let pidl = shell::Pidl::parse(&entry.path).unwrap();
        let parent = pidl.parent().unwrap().folder().unwrap();
        let provider: IShellIcon = parent.cast().unwrap();
        let checked = unsafe {
            provider.GetIconOf(pidl.last_id(), GIL_FORSHELL | GIL_CHECKSHIELD)
        }
        .unwrap();
        let unchecked = unsafe { provider.GetIconOf(pidl.last_id(), GIL_FORSHELL) }.unwrap();
        assert_ne!(checked, unchecked, "Shell did not supply the UAC variant");
        let shield = shell::icons::icon(checked as u32, 16).unwrap();
        let unshielded = shell::icons::icon(unchecked as u32, 16).unwrap();
        assert_eq!(immediate.pixels, shield.pixels);
        assert!(
            immediate
                .pixels
                .chunks_exact(4)
                .zip(unshielded.pixels.chunks_exact(4))
                .any(|(checked, unchecked)| checked[3] != 0 && checked != unchecked),
            "UAC shield is absent"
        );
    }

    #[test]
    fn bad_system_icon_falls_back_and_the_next_row_still_decodes() {
        use super::*;
        assert!(shell::icons::icon(u32::MAX, 16).is_err());
        let root = std::env::temp_dir().join(format!(
            "sw-native-icon-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("folder")).unwrap();
        std::fs::write(root.join("dummy.exe"), [0]).unwrap();
        let mut entries = shell::listing::list(&root.to_string_lossy()).unwrap();
        let file = entries.iter_mut().find(|e| !e.is_dir).unwrap();
        file.icon = u32::MAX;
        file.icon_pending = false;
        let mut icons = RowIcons::default();
        let (index, image) = row_icon(file, &mut icons);
        assert_eq!(index, shell::icons::generic_icon(&file.path));
        assert!(image.unwrap().pixels.chunks_exact(4).any(|p| p[3] > 0));
        let folder = entries.iter().find(|e| e.is_dir).unwrap();
        let (_, decoded) = row_icon(folder, &mut icons);
        assert!(decoded.unwrap().pixels.chunks_exact(4).any(|p| p[3] > 0));
        std::fs::remove_file(root.join("dummy.exe")).unwrap();
        std::fs::remove_dir(root.join("folder")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}

fn network_notice(path: &str) -> Option<String> {
    if !crate::app::ids::same(path, crate::app::ids::NETWORK) {
        return None;
    }
    match shell::network::status() {
        Ok(status) => status
            .resource()
            .map(|resource| crate::i18n::system_string("NetworkExplorer.dll", resource)),
        Err(error) => {
            eprintln!("native discovery status query: {error}");
            None
        }
    }
}

fn load(path: &str, session: Option<crate::app::view::ResolvedView>) -> Result<Folder, String> {
    let title = super::titlebar::TitleBar::new(path).map_err(|error| error.to_string())?;
    let search = crate::app::ids::search_parts(path);
    let settings_page = crate::app::ids::is_settings(path);
    let mut entries =
        if search.is_some() || settings_page || discovery(path) || shell::browser::handles(path) {
            Vec::new()
        } else {
            shell::listing::list(path)?
        };
    entries.shrink_to_fit();
    super::memory::checkpoint("shell-listed");
    let view = search.map_or_else(
        || {
            if settings_page {
                shell::viewstate::ViewState::default()
            } else {
                shell::viewstate::read(path)
            }
        },
        |(scope, _)| shell::viewstate::read_search(scope),
    );
    let view = session.unwrap_or_else(|| {
        crate::app::view::resolve(path, &view, &crate::app::FolderView::default())
    });
    let columns = columns::shown_columns(view.columns.clone());
    let children = entries
        .iter()
        .filter(|e| !e.child.is_empty())
        .map(|e| (e.path.clone(), e.child.clone()))
        .collect();
    let items = shell::columns::Items::new(path, children);
    let order = (0..entries.len()).collect();
    let settings = shell::listing::settings();
    super::memory::checkpoint("shell-view-sorted");
    let mut folder = Folder {
        crumbs: Arc::new(if settings_page {
            vec![shell::listing::Crumb {
                id: path.to_owned(),
                name: crate::i18n::tr("settings.title").into(),
                icon: 0,
                nav_root: None,
                has_subfolders: false,
            }]
        } else if let Some((scope, _)) = search {
            let mut crumbs = shell::listing::crumbs(scope)?;
            let name = crumbs.last().map_or("", |crumb| crumb.name.as_str());
            let name = crate::i18n::shell_string(34132, name);
            crumbs.push(shell::listing::Crumb {
                id: path.to_owned(),
                name,
                icon: shell::icons::find_icon(),
                nav_root: None,
                has_subfolders: false,
            });
            crumbs
        } else {
            shell::listing::crumbs(path)?
        }),
        title: title.title,
        icon: if search.is_some() {
            shell::icons::icon(shell::icons::find_icon(), 16)?
        } else {
            title.icon
        },
        entries: Arc::new(entries),
        headers: Arc::new(Vec::new()),
        collapsed: HashSet::new(),
        layout: Arc::new(crate::app::group::DetailsLayout::default()),
        grid: None,
        gallery_filter: None,
        home_filter: 0,
        filter: None,
        network_notice: network_notice(path),
        library_origins: Arc::new(if shell::library::is_library(path) {
            shell::library::origins(path)
        } else {
            Vec::new()
        }),
        order: Arc::new(order),
        columns,
        view,
        settings,
        panes: shell::listing::panes(),
        pane_width: 302.0,
    };
    arrange(&mut folder, &items);
    Ok(folder)
}

fn arrange(folder: &mut Folder, items: &shell::columns::Items) {
    let mut values = HashMap::new();
    if let Some(key) = folder.view.sort.key.property() {
        for entry in folder.entries.iter() {
            values.insert(
                entry.path.clone(),
                shell::columns::sort_value(items, &entry.path, &key),
            );
        }
    }
    let home = folder
        .crumbs
        .last()
        .is_some_and(|crumb| crate::app::ids::same(&crumb.id, crate::app::ids::HOME));
    let mut order = (0..folder.entries.len())
        .filter(|&index| {
            !home || crate::app::home::matches(&folder.entries[index], folder.home_filter)
        })
        .collect::<Vec<_>>();
    sort::order(
        &folder.entries,
        &mut order,
        folder.view.sort,
        &folder.view.extra_sorts,
        &values,
        folder
            .crumbs
            .last()
            .is_some_and(|crumb| sort::folders_first(&crumb.id)),
        folder
            .crumbs
            .last()
            .is_some_and(|crumb| crate::app::ids::is_search(&crumb.id)),
    );
    if folder.view.search_ranked {
        order.sort_by_key(|&index| !folder.entries[index].is_dir);
    }
    group_order(folder, items, order);
}

fn group_order(folder: &mut Folder, items: &shell::columns::Items, mut order: Vec<usize>) {
    let path = folder.crumbs.last().map_or("", |crumb| crumb.id.as_str());
    if crate::app::ids::same(path, crate::app::ids::GALLERY) {
        crate::app::gallery::order(
            &folder.entries,
            &mut order,
            folder.gallery_filter.as_deref(),
        );
    }
    if let Some(filter) = &folder.filter {
        let today = crate::app::group::Today::now();
        order.retain(|&index| {
            let entry = &folder.entries[index];
            let value = filter
                .key
                .property()
                .map(|key| shell::columns::sort_value(items, &entry.path, &key));
            filter.accepts(entry, &today, value.as_ref())
        });
    }
    let drives = crate::app::ids::same(path, crate::app::ids::THIS_PC) && folder.view.mode == 7;
    let headers = if drives {
        order.sort_by_key(|&i| !crate::app::drives_layout::is_device(&folder.entries[i]));
        let mut headers = vec![String::new(); order.len()];
        let mut previous = None;
        for (row, &i) in order.iter().enumerate() {
            let device = crate::app::drives_layout::is_device(&folder.entries[i]);
            if previous != Some(device) {
                headers[row] = crate::i18n::tr(if device {
                    "tiles.drives"
                } else {
                    "tiles.network"
                })
                .into();
                previous = Some(device);
            }
        }
        headers
    } else if let Some(group) = folder.view.group {
        let today = crate::app::group::Today::now();
        let buckets = order
            .iter()
            .map(|&index| {
                let entry = &folder.entries[index];
                let value = group
                    .key
                    .property()
                    .map(|key| shell::columns::sort_value(items, &entry.path, &key));
                (
                    index,
                    if crate::app::ids::same(path, crate::app::ids::THIS_PC)
                        && group.key == crate::app::sort::SortKey::Type
                    {
                        crate::app::drives_layout::bucket(entry)
                    } else if folder.library_grouped() && entry.library_origin.is_some() {
                        let origin = entry.library_origin.as_ref().unwrap();
                        crate::app::group::Bucket {
                            rank: folder
                                .library_origins
                                .iter()
                                .position(|source| {
                                    crate::app::ids::same(&source.path, &origin.path)
                                })
                                .unwrap_or(0) as i32,
                            label: crate::app::library_view::label(origin),
                        }
                    } else {
                        crate::app::group::bucket(group.key, entry, &today, value.as_ref())
                    },
                )
            })
            .collect();
        crate::app::group::arrange(&mut order, &buckets, group)
    } else {
        Vec::new()
    };
    folder.order = Arc::new(order);
    folder.headers = Arc::new(headers);
    folder.rebuild_layout();
}
