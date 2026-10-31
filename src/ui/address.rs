use std::sync::{mpsc, Arc};
use windows::core::{Interface, Result};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use super::render::Canvas;
use super::text::{Family, TextEngine, TextStyle};
use super::theme::Theme;
use crate::shell::listing::Crumb;

pub const READY: u32 = WM_APP + 12;
pub const LABEL: TextStyle = TextStyle::new(Family::Text, 14.0, 400);
pub const EDIT: TextStyle = TextStyle::new(Family::Small, 14.0, 400);
const GLYPH: TextStyle = TextStyle::new(Family::Icons, 12.0, 400);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Back,
    Forward,
    Up,
    Refresh,
    Root,
    Cloud,
    More,
    Crumb(usize),
    Chevron(usize),
    Address,
    AddressClear,
    Search,
    SearchClear,
    Settings,
}

#[derive(Clone)]
pub struct Item {
    pub label: String,
    pub path: String,
}

pub enum Request {
    ZoxideDetect(bool),
    ZoxideInstallationChanged,
    DragPicture(u64, String),
    Infotip(u64, String),
    Navigation(super::navigation::Work),
    Path(String),
    Suggestions(u64, String),
    Children(u64, Target, String),
    Submit(u64, String, String, String),
    Jump(u64, String, String),
    Stop,
}

pub enum Reply {
    ZoxideInstallationChanged,
    DragPicture(
        u64,
        std::result::Result<(crate::shell::icons::Rgba, crate::shell::icons::Rgba), String>,
    ),
    Infotip(u64, String),
    Suggestions(u64, Vec<String>, Vec<String>),
    Children(u64, Target, std::result::Result<Vec<Item>, String>),
    Submitted(u64, String, String, Option<crate::app::address::Submission>),
    Jumped(u64, String, String, Option<String>),
    Cloud(crate::shell::cloud::Snapshot, Option<Arc<Vec<u8>>>),
}

pub struct Worker {
    sender: mpsc::Sender<Request>,
    receiver: mpsc::Receiver<Reply>,
    cloud: Arc<std::sync::Mutex<Option<crate::shell::cloud::Sender>>>,
    observed_path: std::cell::RefCell<Option<String>>,
}

impl Worker {
    pub fn start(hwnd: HWND) -> std::io::Result<Self> {
        let (sender, requests) = mpsc::channel();
        let (replies, receiver) = mpsc::channel();
        let handle = hwnd.0 as usize;
        let cloud = Arc::new(std::sync::Mutex::new(None));
        let shared_cloud = cloud.clone();
        std::thread::Builder::new()
            .name("address-shell".into())
            .spawn(move || {
                crate::shell::com_init();
                let mut pending = None;
                while let Some(request) = next_request(&requests, &mut pending) {
                    let reply = match request {
                        Request::ZoxideDetect(enable) => {
                            crate::app::zoxide_install::detect(enable);
                            Reply::ZoxideInstallationChanged
                        }
                        Request::ZoxideInstallationChanged => Reply::ZoxideInstallationChanged,
                        Request::DragPicture(token, path) => Reply::DragPicture(
                            token,
                            crate::shell::icons::tab_drag_picture(&path, 80).map(|tab| {
                                let crumb = match crate::shell::icons::shell_picture(&path, 80) {
                                    Ok(picture) => picture,
                                    Err(error) => {
                                        eprintln!("crumb drag picture: {error}");
                                        tab.clone()
                                    }
                                };
                                (crumb, tab)
                            }),
                        ),
                        Request::Infotip(token, path) => Reply::Infotip(
                            token,
                            crate::shell::listing::info_tip(&path).join("\r\n"),
                        ),
                        Request::Navigation(work) => {
                            work.run();
                            continue;
                        }
                        Request::Path(path) => {
                            crate::app::zoxide::record(&path);
                            let mut source = match shared_cloud.lock() {
                                Ok(source) => source,
                                Err(error) => {
                                    eprintln!("cloud channel: {error}");
                                    break;
                                }
                            };
                            if crate::shell::cloud::supports(&path) {
                                if source.is_none() {
                                    match cloud_worker(replies.clone(), handle) {
                                        Ok(sender) => *source = Some(sender),
                                        Err(error) => {
                                            eprintln!("cloud startup: {error}");
                                            continue;
                                        }
                                    }
                                }
                                if let Err(error) = source
                                    .as_ref()
                                    .unwrap()
                                    .send(crate::shell::cloud::Request::Path(path))
                                {
                                    eprintln!("cloud path: {error}");
                                }
                                continue;
                            }
                            source.take();
                            Reply::Cloud(
                                crate::shell::cloud::Snapshot {
                                    path,
                                    ..Default::default()
                                },
                                None,
                            )
                        }

                        Request::Stop => break,
                        Request::Suggestions(generation, text) => {
                            let mut items = crate::shell::address::suggestions(&text);
                            crate::app::zoxide::suggestions(&text, &mut items);
                            Reply::Suggestions(
                                generation,
                                crate::shell::address::typed_paths(),
                                items,
                            )
                        }
                        Request::Children(generation, target, path) => Reply::Children(
                            generation,
                            target,
                            crate::shell::listing::subfolders(&path).map(|items| {
                                items
                                    .into_iter()
                                    .map(|(label, path)| Item { label, path })
                                    .collect()
                            }),
                        ),
                        Request::Submit(generation, text, path, location) => {
                            let submission = crate::app::address::resolve(&text, &path, &location);
                            Reply::Submitted(generation, text, path, submission)
                        }
                        Request::Jump(generation, text, path) => {
                            let target = crate::app::zoxide::jump(&text);
                            Reply::Jumped(generation, text, path, target)
                        }
                    };
                    if replies.send(reply).is_err() {
                        break;
                    }
                    if let Err(error) = unsafe {
                        PostMessageW(Some(HWND(handle as _)), READY, WPARAM(0), LPARAM(0))
                    } {
                        eprintln!("address notification: {error}");
                        break;
                    }
                }
            })?;
        Ok(Self {
            sender,
            receiver,
            cloud,
            observed_path: Default::default(),
        })
    }

    pub fn send(&self, request: Request) {
        if let Err(error) = self.sender.send(request) {
            eprintln!("address request: {error}");
        }
    }

    pub fn dispatcher(&self) -> mpsc::Sender<Request> {
        self.sender.clone()
    }

    pub fn cloud_sender(&self) -> Option<crate::shell::cloud::Sender> {
        match self.cloud.lock() {
            Ok(source) => source.clone(),
            Err(error) => {
                eprintln!("cloud channel: {error}");
                None
            }
        }
    }

    pub fn receive(&self) -> Vec<Reply> {
        self.receiver.try_iter().collect()
    }

    pub fn path(&self, path: &str) {
        if self.observed_path.borrow().as_deref() == Some(path) {
            return;
        }
        *self.observed_path.borrow_mut() = Some(path.to_string());
        self.send(Request::Path(path.into()));
    }
}

fn cloud_worker(
    cloud_replies: mpsc::Sender<Reply>,
    handle: usize,
) -> std::result::Result<crate::shell::cloud::Sender, String> {
    crate::shell::cloud::start(move |snapshot| {
        let bytes = crate::shell::cloud::icon_path(&snapshot.icon).and_then(|path| {
            match std::fs::read(&path) {
                Ok(bytes) => Some(Arc::new(bytes)),
                Err(error) => {
                    eprintln!("cloud icon {}: {error}", path.display());
                    None
                }
            }
        });
        if cloud_replies.send(Reply::Cloud(snapshot, bytes)).is_ok() {
            if let Err(error) =
                unsafe { PostMessageW(Some(HWND(handle as _)), READY, WPARAM(0), LPARAM(0)) }
            {
                eprintln!("cloud notification: {error}");
            }
        }
    })
}

fn next_request(
    requests: &mpsc::Receiver<Request>,
    pending: &mut Option<Request>,
) -> Option<Request> {
    let mut request = pending.take().or_else(|| requests.recv().ok())?;
    if matches!(&request, Request::Suggestions(..) | Request::Infotip(..)) {
        let kind = std::mem::discriminant(&request);
        while let Ok(next) = requests.try_recv() {
            if std::mem::discriminant(&next) == kind {
                request = next;
            } else {
                *pending = Some(next);
                break;
            }
        }
    }
    Some(request)
}

#[cfg(test)]
mod worker_tests {
    use super::*;

    #[test]
    fn hovering_many_items_keeps_the_latest_tip_without_dropping_submissions() {
        let (sender, receiver) = mpsc::channel();
        sender.send(Request::Infotip(1, "alpha".into())).unwrap();
        sender.send(Request::Infotip(2, "beta".into())).unwrap();
        sender
            .send(Request::Submit(
                1,
                "text".into(),
                "path".into(),
                "location".into(),
            ))
            .unwrap();
        sender.send(Request::Infotip(3, "gamma".into())).unwrap();
        let mut pending = None;
        assert!(
            matches!(next_request(&receiver, &mut pending), Some(Request::Infotip(2, path)) if path == "beta")
        );
        assert!(
            matches!(next_request(&receiver, &mut pending), Some(Request::Submit(1, text, path, location)) if text == "text" && path == "path" && location == "location")
        );
        assert!(
            matches!(next_request(&receiver, &mut pending), Some(Request::Infotip(3, path)) if path == "gamma")
        );
    }

    #[test]
    fn newer_suggestions_skip_obsolete_work_without_crossing_a_submission() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Request::Suggestions(1, "C:\\W".into()))
            .unwrap();
        sender
            .send(Request::Suggestions(2, "C:\\Win".into()))
            .unwrap();
        sender
            .send(Request::Submit(
                1,
                "C:\\Windows".into(),
                "C:\\".into(),
                "C:".into(),
            ))
            .unwrap();
        sender.send(Request::Suggestions(3, "D:".into())).unwrap();
        sender.send(Request::Stop).unwrap();
        let mut pending = None;
        assert!(matches!(
            next_request(&receiver, &mut pending),
            Some(Request::Suggestions(2, _))
        ));
        assert!(matches!(
            next_request(&receiver, &mut pending),
            Some(Request::Submit(..))
        ));
        assert!(matches!(
            next_request(&receiver, &mut pending),
            Some(Request::Suggestions(3, _))
        ));
        assert!(matches!(
            next_request(&receiver, &mut pending),
            Some(Request::Stop)
        ));
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Err(error) = self.sender.send(Request::Stop) {
            eprintln!("address stop: {error}");
        }
    }
}

#[derive(Default)]
pub struct State {
    pub path: String,
    pub location: String,
    pub crumbs: Arc<Vec<Crumb>>,
    pub hidden: usize,
    pub drop_crumb: Option<usize>,
    pub hover: Option<Target>,
    pub pressed: Option<Target>,
    pub open: Option<Target>,
    pub focus: Option<Target>,
    pub cues: bool,
    pub enabled: [bool; 4],
    pub editing: bool,
    pub automatic_history: bool,
    pub search_focus: bool,
    pub address_edit: super::edit::State,
    pub search_edit: super::edit::State,
    pub generation: u64,
    pub suggestions: Vec<String>,
    pub typed_paths: Vec<String>,
    pub suggestion: Option<usize>,
    pub typed: String,
    pub drag: Option<(Target, u32)>,
    pub drag_x: f32,
    pub cloud: Option<crate::shell::cloud::Snapshot>,
    pub cloud_image: Option<Arc<Vec<u8>>>,
    pub boxes: Vec<(Target, D2D_RECT_F)>,
    pub width: f32,
    pub address_width: f32,
    pub search_left: f32,
}

impl State {
    pub fn sync(&mut self, tab: &crate::app::Tab, folder: Option<&super::list::Folder>) {
        self.enabled = [
            tab.index > 0,
            tab.index + 1 < tab.history.len(),
            !tab.path().is_empty() && !crate::app::ids::is_settings(tab.path()),
            true,
        ];
        if self.path != tab.path() {
            self.generation += 1;
            self.cloud = None;
            self.cloud_image = None;
        }
        self.path = tab.path().to_owned();
        if let Some(folder) = folder {
            self.location = folder.title.clone();
            self.crumbs = folder.crumbs.clone();
        }
    }

    pub fn layout(&mut self, width: f32, text: &mut TextEngine) -> Result<()> {
        self.width = width;
        let total = (width - 220.0).max(0.0);
        let original_search = ((total - 37.333) * 0.3).round().max(0.0);
        let search = (original_search - 40.0).max(0.0);
        self.address_width = total - original_search;
        self.search_left = 201.0 + self.address_width + 8.0;
        self.address_edit.field = rect(
            212.0,
            8.0,
            (self.address_width
                - 11.0
                - if self.address_edit.text.is_empty() {
                    11.0
                } else {
                    40.0
                })
            .max(1.0),
            32.0,
        );
        self.search_edit.field = rect(self.search_left + 11.0, 8.0, (search - 80.0).max(1.0), 32.0);
        self.boxes.clear();
        for (index, target) in [Target::Back, Target::Forward, Target::Up, Target::Refresh]
            .into_iter()
            .enumerate()
        {
            self.boxes
                .push((target, rect(11.0 + 48.0 * index as f32, 8.0, 32.0, 32.0)));
        }
        self.boxes
            .push((Target::Settings, rect(width - 43.0, 8.0, 32.0, 32.0)));
        self.boxes
            .push((Target::Address, rect(201.0, 8.0, self.address_width, 32.0)));
        self.boxes
            .push((Target::Search, rect(self.search_left, 8.0, search, 32.0)));
        if self.editing {
            if !self.address_edit.text.is_empty() {
                self.boxes.push((
                    Target::AddressClear,
                    rect(201.0 + self.address_width - 35.0, 13.0, 30.0, 22.0),
                ));
            }
        } else {
            let root_width = if let Some(cloud) = &self.cloud {
                let width = text.layout(&cloud.label, LABEL, 4096.0)?.advance.ceil() + 42.0;
                self.boxes
                    .push((Target::Cloud, rect(204.0, 12.0, width, 24.0)));
                self.boxes
                    .push((Target::Root, rect(203.0 + width, 12.0, 26.0, 24.0)));
                width + 28.0
            } else {
                self.boxes
                    .push((Target::Root, rect(239.0, 12.0, 26.0, 24.0)));
                64.0
            };
            let widths = self
                .crumbs
                .iter()
                .enumerate()
                .map(|(index, crumb)| {
                    text.layout(&crumb.name, LABEL, 4096.0).map(|layout| {
                        layout.advance.ceil() + 18.0 + if self.chevron(index) { 23.0 } else { 0.0 }
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let search = crate::app::ids::is_search(&self.path);
            self.hidden = if search {
                self.crumbs.len().saturating_sub(1)
            } else {
                crate::app::address::hidden_crumbs(&widths, self.address_width - root_width - 4.0)
            };
            let mut left = 201.0 + root_width;
            if self.hidden > 0 && !search {
                self.boxes
                    .push((Target::More, rect(left, 12.0, 31.0, 24.0)));
                left += 31.0;
            }
            for (index, width) in widths.into_iter().enumerate().skip(self.hidden) {
                let label = width - if self.chevron(index) { 23.0 } else { 0.0 };
                self.boxes
                    .push((Target::Crumb(index), rect(left, 12.0, label, 24.0)));
                if self.chevron(index) {
                    self.boxes.push((
                        Target::Chevron(index),
                        rect(left + label - 1.0, 12.0, 24.0, 24.0),
                    ));
                }
                left += width;
            }
        }
        if !self.search_edit.text.is_empty() {
            self.boxes
                .push((Target::SearchClear, rect(width - 120.0, 13.0, 30.0, 22.0)));
        }
        Ok(())
    }

    fn chevron(&self, index: usize) -> bool {
        self.crumbs.get(index).is_some_and(|crumb| {
            crate::app::ids::crumb_has_children(&crumb.id)
                && (index + 1 < self.crumbs.len() || crumb.has_subfolders)
        })
    }

    pub fn hit(&self, point: (f32, f32)) -> Option<Target> {
        if super::details::top() == 0.0 {
            return None;
        }
        self.boxes
            .iter()
            .rev()
            .find_map(|(target, bounds)| {
                if matches!(
                    target,
                    Target::Crumb(_)
                        | Target::Chevron(_)
                        | Target::More
                        | Target::Root
                        | Target::Cloud
                ) && point.0 >= 201.0 + self.address_width
                {
                    return None;
                }
                (contains(*bounds, (point.0, point.1 - 40.0))
                    && !matches!(
                        target,
                        Target::Back | Target::Forward | Target::Up | Target::Refresh
                    ))
                .then_some(*target)
            })
            .or_else(|| {
                self.boxes
                    .iter()
                    .take(4)
                    .enumerate()
                    .find_map(|(index, (target, bounds))| {
                        (self.enabled[index] && contains(*bounds, (point.0, point.1 - 40.0)))
                            .then_some(*target)
                    })
            })
    }

    pub fn bounds(&self, target: Target) -> Option<D2D_RECT_F> {
        self.boxes
            .iter()
            .find(|(item, _)| *item == target)
            .map(|(_, bounds)| *bounds)
    }
}

pub fn rect(x: f32, y: f32, width: f32, height: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    }
}

pub fn contains(bounds: D2D_RECT_F, point: (f32, f32)) -> bool {
    point.0 >= bounds.left
        && point.0 < bounds.right
        && point.1 >= bounds.top
        && point.1 < bounds.bottom
}

#[derive(Default)]
pub struct ProviderIcon {
    source: Option<(Arc<Vec<u8>>, u32)>,
    bitmap: Option<ID2D1Bitmap1>,
}

impl ProviderIcon {
    pub fn draw(&self, canvas: &Canvas, bounds: D2D_RECT_F, opacity: f32) {
        if let Some(bitmap) = &self.bitmap {
            unsafe {
                canvas.context.DrawBitmap(
                    bitmap,
                    Some(&bounds),
                    opacity,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                );
            }
        }
    }

    pub fn prepare(
        &mut self,
        canvas: &Canvas,
        source: Option<&Arc<Vec<u8>>>,
        dpi: u32,
    ) -> Result<()> {
        let Some(source) = source else {
            self.source = None;
            self.bitmap = None;
            return Ok(());
        };
        if self
            .source
            .as_ref()
            .is_some_and(|(bytes, old_dpi)| old_dpi == &dpi && bytes == source)
        {
            return Ok(());
        }
        let create = (|| -> Result<ID2D1Bitmap1> {
            unsafe {
                let context: ID2D1DeviceContext5 = canvas.context.cast()?;
                let stream = windows::Win32::UI::Shell::SHCreateMemStream(Some(source))
                    .ok_or_else(windows::core::Error::from_win32)?;
                let document = context.CreateSvgDocument(
                    &stream,
                    D2D_SIZE_F {
                        width: 16.0,
                        height: 16.0,
                    },
                )?;
                let edge = (16.0 * dpi as f32 / 96.0).round().max(1.0) as u32;
                let bitmap = canvas.context.CreateBitmap(
                    D2D_SIZE_U {
                        width: edge,
                        height: edge,
                    },
                    None,
                    0,
                    &D2D1_BITMAP_PROPERTIES1 {
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format:
                                windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                        },
                        dpiX: 96.0,
                        dpiY: 96.0,
                        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                        ..Default::default()
                    },
                )?;
                canvas.context.SetTarget(&bitmap);
                canvas.context.SetDpi(dpi as f32, dpi as f32);
                canvas.context.BeginDraw();
                canvas.context.Clear(Some(&D2D1_COLOR_F::default()));
                context.DrawSvgDocument(&document);
                let result = canvas.context.EndDraw(None, None);
                canvas.context.SetTarget(None);
                canvas.context.SetDpi(96.0, 96.0);
                result?;
                Ok(bitmap)
            }
        })();
        match create {
            Ok(bitmap) => self.bitmap = Some(bitmap),
            Err(error) if super::render::device_lost(&error) => return Err(error),
            Err(error) => {
                eprintln!("cloud provider icon: {error}");
                self.bitmap = None;
            }
        }
        self.source = Some((source.clone(), dpi));
        Ok(())
    }
}

pub fn rounded(canvas: &Canvas, bounds: D2D_RECT_F, radius: f32, color: &D2D1_COLOR_F) {
    unsafe {
        canvas
            .context
            .SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        canvas.brush.SetColor(color);
        canvas.context.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: bounds,
                radiusX: radius,
                radiusY: radius,
            },
            &canvas.brush,
        );
    }
}

fn glyph(
    canvas: &Canvas,
    text: &mut TextEngine,
    value: &str,
    bounds: D2D_RECT_F,
    color: &D2D1_COLOR_F,
) -> Result<()> {
    let layout = text.layout(value, GLYPH, 4096.0)?;
    text.draw(
        canvas,
        value,
        GLYPH,
        (
            (bounds.left + bounds.right - layout.advance) / 2.0,
            (bounds.top + bounds.bottom - layout.height) / 2.0,
        ),
        color,
    )
}

fn chevron(
    canvas: &Canvas,
    text: &mut TextEngine,
    bounds: D2D_RECT_F,
    color: &D2D1_COLOR_F,
    open: Option<Target>,
) -> Result<()> {
    if open.is_none() {
        return glyph(canvas, text, "\u{e974}", bounds, color);
    }
    let layout = text.layout("\u{e974}", GLYPH, 4096.0)?;
    let x = (bounds.left + bounds.right + layout.height) / 2.0;
    let y = (bounds.top + bounds.bottom - layout.advance) / 2.0;
    let mut original = Default::default();
    unsafe {
        canvas.context.GetTransform(&mut original);
        let mut rotated = original;
        rotated.M11 = original.M21;
        rotated.M12 = original.M22;
        rotated.M21 = -original.M11;
        rotated.M22 = -original.M12;
        rotated.M31 = original.M11 * x + original.M21 * y + original.M31;
        rotated.M32 = original.M12 * x + original.M22 * y + original.M32;
        canvas.context.SetTransform(&rotated);
    }
    let result = text.draw(canvas, "\u{e974}", GLYPH, (0.0, 0.0), color);
    unsafe {
        canvas.context.SetTransform(&original);
    }
    result
}

pub fn draw(
    state: &mut State,
    assets: &mut super::svg::Assets,
    provider: &ProviderIcon,
    canvas: &Canvas,
    text: &mut TextEngine,
    theme: &Theme,
) -> Result<()> {
    super::details::fill(canvas, rect(0.0, 0.0, state.width, 48.0), &theme.layer);
    let address = rect(201.0, 8.0, state.address_width, 32.0);
    let search = rect(
        state.search_left,
        8.0,
        (state.width - 51.0 - state.search_left).max(0.0),
        32.0,
    );
    rounded(
        canvas,
        address,
        4.0,
        if state.editing {
            &theme.address_edit
        } else {
            &theme.control
        },
    );
    rounded(
        canvas,
        search,
        4.0,
        if state.search_focus {
            &theme.search_focus
        } else if state.hover == Some(Target::Search) {
            &theme.control_hover
        } else {
            &theme.control
        },
    );
    if let Some(bounds) = state.bounds(Target::Settings) {
        if state.hover == Some(Target::Settings) {
            rounded(
                canvas,
                bounds,
                4.0,
                if state.pressed == Some(Target::Settings) {
                    &theme.subtle_pressed
                } else {
                    &theme.subtle_hover
                },
            );
        }
        glyph(canvas, text, "\u{e713}", bounds, &theme.text)?;
        if state.cues && state.focus == Some(Target::Settings) {
            super::chrome::focus_ring_radius(
                canvas,
                rect(bounds.left - 3.0, bounds.top - 3.0, 38.0, 38.0),
                theme,
                4.0,
            );
        }
    }
    for (index, icon) in ["\u{e72b}", "\u{e72a}", "\u{e110}", "\u{e72c}"]
        .into_iter()
        .enumerate()
    {
        let (target, bounds) = state.boxes[index];
        if state.enabled[index] && state.hover == Some(target) {
            rounded(
                canvas,
                bounds,
                4.0,
                if state.pressed == Some(target) {
                    &theme.subtle_pressed
                } else {
                    &theme.subtle_hover
                },
            );
        }
        glyph(
            canvas,
            text,
            icon,
            bounds,
            if !state.enabled[index] {
                &theme.text_disabled
            } else if state.pressed == Some(target) {
                &theme.text_secondary
            } else {
                &theme.text
            },
        )?;
        if state.cues && state.focus == Some(target) {
            super::chrome::focus_ring(
                canvas,
                rect(bounds.left - 3.0, bounds.top - 3.0, 38.0, 38.0),
                theme,
            );
        }
    }
    if state.editing {
        let field = rect(
            address.left + 11.0,
            8.0,
            (state.address_width
                - 11.0
                - if state.address_edit.text.is_empty() {
                    11.0
                } else {
                    40.0
                })
            .max(1.0),
            32.0,
        );
        state.address_edit.draw(canvas, text, theme, field)?;
        underline(
            canvas,
            address,
            if state.automatic_history { 4.5 } else { 5.0 },
            theme,
        );
    } else {
        let root_bounds = state
            .bounds(Target::Root)
            .unwrap_or(rect(214.0, 12.0, 51.0, 24.0));
        if state.hover == Some(Target::Root) || state.open == Some(Target::Root) {
            rounded(
                canvas,
                root_bounds,
                4.0,
                if state.pressed == Some(Target::Root) {
                    &theme.subtle_pressed
                } else {
                    &theme.subtle_hover
                },
            );
        }
        let root_color = if state.pressed == Some(Target::Root) {
            &theme.text_secondary
        } else {
            &theme.text
        };
        let trail = if let (Some(cloud), Some(bounds)) = (&state.cloud, state.bounds(Target::Cloud))
        {
            if state.hover == Some(Target::Cloud) {
                rounded(canvas, bounds, 4.0, &theme.subtle_hover);
            }
            if let Some(bitmap) = &provider.bitmap {
                unsafe {
                    canvas.context.DrawBitmap(
                        bitmap,
                        Some(&rect(bounds.left + 8.0, 16.0, 16.0, 16.0)),
                        1.0,
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        None,
                        None,
                    );
                }
            }
            text.draw(
                canvas,
                &cloud.label,
                LABEL,
                (bounds.left + 32.0, 13.0),
                &theme.text,
            )?;
            chevron(
                canvas,
                text,
                root_bounds,
                root_color,
                state.open.filter(|target| *target == Target::Root),
            )?;
            root_bounds.right
        } else {
            let root = state
                .crumbs
                .first()
                .map(|crumb| crate::app::ids::root_kind(&crumb.id))
                .unwrap_or(1);
            let name = match root {
                2 => "addressbar.home",
                3 => "addressbar.gallery",
                4 => "addressbar.network",
                _ => "addressbar.desktop",
            };
            if crate::app::ids::is_settings(&state.path) {
                glyph(
                    canvas,
                    text,
                    "\u{e713}",
                    rect(212.0, 12.0, 20.0, 24.0),
                    &theme.text,
                )?;
            } else {
                assets.icon(canvas, name, (214.0, 16.0), 1.0)?;
            }
            chevron(
                canvas,
                text,
                root_bounds,
                root_color,
                state.open.filter(|target| *target == Target::Root),
            )?;
            265.0
        };
        if state.cues && state.focus == Some(Target::Root) {
            super::chrome::focus_ring_radius(canvas, rect(203.0, 8.0, 62.0, 32.0), theme, 4.0);
        }
        unsafe {
            canvas.context.PushAxisAlignedClip(
                &rect(
                    trail - 2.0,
                    8.0,
                    (201.0 + state.address_width - trail - 1.0).max(0.0),
                    32.0,
                ),
                D2D1_ANTIALIAS_MODE_ALIASED,
            );
        }
        if let Some(index) = state.drop_crumb {
            if let Some(mut bounds) = state.bounds(Target::Crumb(index)) {
                if let Some(chevron) = state.bounds(Target::Chevron(index)) {
                    bounds.right = chevron.right;
                }
                rounded(canvas, bounds, 4.0, &theme.subtle_hover);
            }
        }
        for &(target, bounds) in state.boxes.iter().skip(7) {
            if !matches!(target, Target::Crumb(_) | Target::Chevron(_) | Target::More) {
                continue;
            }
            if state.hover == Some(target) || state.open == Some(target) {
                let plate = if state.open == Some(target) && matches!(target, Target::Chevron(_)) {
                    rect(
                        bounds.left + 1.0,
                        bounds.top + 1.0,
                        bounds.right - bounds.left - 2.0,
                        bounds.bottom - bounds.top - 2.0,
                    )
                } else {
                    bounds
                };
                rounded(
                    canvas,
                    plate,
                    4.0,
                    if state.pressed == Some(target) {
                        &theme.subtle_pressed
                    } else {
                        &theme.subtle_hover
                    },
                );
            }
            let color = if state.pressed == Some(target) {
                &theme.text_secondary
            } else {
                &theme.text
            };
            match target {
                Target::Crumb(index) => text.draw(
                    canvas,
                    &state.crumbs[index].name,
                    LABEL,
                    (bounds.left + 9.0, 14.0),
                    color,
                )?,
                Target::Chevron(index) => chevron(
                    canvas,
                    text,
                    rect(
                        bounds.left
                            + 0.5
                            + if index + 1 == state.crumbs.len() {
                                1.0
                            } else {
                                0.0
                            },
                        bounds.top,
                        23.0,
                        24.0,
                    ),
                    color,
                    state.open.filter(|open| *open == target),
                )?,
                Target::More => {
                    let style = TextStyle::new(Family::Icons, 14.0, 400);
                    let layout = text.layout("\u{e712}", style, 4096.0)?;
                    text.draw(
                        canvas,
                        "\u{e712}",
                        style,
                        (
                            (bounds.left + bounds.right - layout.advance) / 2.0 + 0.5,
                            (bounds.top + bounds.bottom - layout.height) / 2.0 - 1.0,
                        ),
                        color,
                    )?;
                }
                _ => {}
            }
            if state.cues && state.focus == Some(target) {
                super::chrome::focus_ring(
                    canvas,
                    rect(
                        bounds.left - 2.0,
                        bounds.top - 2.0,
                        bounds.right - bounds.left + 4.0,
                        28.0,
                    ),
                    theme,
                );
            }
        }
        unsafe {
            canvas.context.PopAxisAlignedClip();
        }
    }
    if state.search_edit.text.is_empty() {
        state
            .search_edit
            .draw(canvas, text, theme, state.search_edit.field)?;
        unsafe {
            canvas.context.PushAxisAlignedClip(
                &rect(
                    search.left + 15.0,
                    8.0,
                    (search.right - search.left - 60.0).max(1.0),
                    32.0,
                ),
                D2D1_ANTIALIAS_MODE_ALIASED,
            );
        }
        text.draw(
            canvas,
            &format!(" {}", crate::i18n::trf("nav.search", &[&state.location])),
            TextStyle::new(Family::Variable, 14.0, 400),
            (search.left + 11.0, 14.0),
            &theme.text_secondary,
        )?;
        unsafe {
            canvas.context.PopAxisAlignedClip();
        }
    } else {
        state.search_edit.draw(
            canvas,
            text,
            theme,
            rect(
                search.left + 11.0,
                8.0,
                (search.right - search.left - 80.0).max(1.0),
                32.0,
            ),
        )?;
    }
    glyph(
        canvas,
        text,
        "\u{e721}",
        rect(search.right - 25.0, 8.0, 12.0, 32.0),
        &theme.text,
    )?;
    if state.search_focus {
        underline(canvas, search, 4.5, theme);
    }
    for target in [Target::AddressClear, Target::SearchClear] {
        if let Some(bounds) = state.bounds(target) {
            if state.hover == Some(target)
                || target == Target::AddressClear && theme.content.r < 0.5
            {
                rounded(
                    canvas,
                    bounds,
                    4.0,
                    if state.pressed == Some(target) {
                        &theme.subtle_pressed
                    } else {
                        &theme.subtle_hover
                    },
                );
            }
            glyph(canvas, text, "\u{e894}", bounds, &theme.text_secondary)?;
        }
    }
    Ok(())
}

fn underline(canvas: &Canvas, bounds: D2D_RECT_F, radius: f32, theme: &Theme) {
    unsafe {
        canvas.context.PushAxisAlignedClip(
            &rect(
                bounds.left,
                bounds.bottom - 2.0,
                bounds.right - bounds.left,
                2.0,
            ),
            D2D1_ANTIALIAS_MODE_ALIASED,
        );
    }
    rounded(canvas, bounds, radius, &theme.accent);
    unsafe {
        canvas.context.PopAxisAlignedClip();
    }
}
