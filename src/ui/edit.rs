use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::{EM_GETSEL, EM_SETSEL};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::address::rect;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    None,
    Typing,
    Backward,
    Forward,
    Replace,
    Composition,
    Movement,
    Programmatic,
    History,
}

#[derive(Clone)]
pub(super) struct Record {
    text: String,
    start: u32,
    end: u32,
    active: u32,
}

struct Group {
    kind: Kind,
    active: u32,
    time: std::time::Instant,
}

#[derive(Default)]
pub struct State {
    pub text: String,
    pub start: u32,
    pub end: u32,
    pub active: u32,
    pub offset: f32,
    pub focused: bool,
    pub revision: u64,
    pub field: D2D_RECT_F,
    pub caret: Option<D2D_RECT_F>,
    undo: Vec<Record>,
    redo: Vec<Record>,
    group: Option<Group>,
}

pub struct Snapshot {
    pub text: String,
    pub start: u32,
    pub end: u32,
    pub active: u32,
    pub focused: bool,
    pub kind: Kind,
}

impl State {
    pub fn update(&mut self, snapshot: Snapshot) -> bool {
        let changed = self.text != snapshot.text;
        let selection_changed = self.start != snapshot.start || self.end != snapshot.end;
        if changed {
            match snapshot.kind {
                Kind::History => self.group = None,
                Kind::Programmatic => self.clear_history(),
                kind => {
                    let now = std::time::Instant::now();
                    let grouped = self.group.as_ref().is_some_and(|group| {
                        group.kind == kind
                            && (kind == Kind::Composition
                                || (matches!(kind, Kind::Typing | Kind::Backward | Kind::Forward)
                                    && self.start == self.end
                                    && group.active == self.active
                                    && now.duration_since(group.time)
                                        < std::time::Duration::from_secs(1)))
                    });
                    if !grouped {
                        if self.undo.len() == 64 {
                            self.undo.remove(0);
                        }
                        self.undo.push(self.record());
                    }
                    self.redo.clear();
                    self.group = matches!(
                        kind,
                        Kind::Typing | Kind::Backward | Kind::Forward | Kind::Composition
                    )
                    .then_some(Group {
                        kind,
                        active: snapshot.active,
                        time: now,
                    });
                }
            }
        } else if selection_changed || snapshot.kind == Kind::Movement || !snapshot.focused {
            self.group = None;
        }
        self.text = snapshot.text;
        self.start = snapshot.start;
        self.end = snapshot.end;
        if changed || selection_changed {
            self.active = snapshot.active;
        }
        self.focused = snapshot.focused;
        self.revision = self.revision.wrapping_add(1);
        changed
    }

    fn record(&self) -> Record {
        Record {
            text: self.text.clone(),
            start: self.start,
            end: self.end,
            active: self.active,
        }
    }

    pub(super) fn take_history(&mut self, key: u32) -> Option<Record> {
        let current = self.record();
        let record = if key == 0x5a {
            let record = self.undo.pop()?;
            self.redo.push(current);
            record
        } else {
            let record = self.redo.pop()?;
            self.undo.push(current);
            record
        };
        self.group = None;
        Some(record)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn clear_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.group = None;
    }

    pub fn hit(&self, engine: &mut super::text::TextEngine, x: f32) -> Result<u32> {
        engine
            .layout(&self.text, super::address::EDIT, 65536.0)?
            .hit(x - self.field.left + self.offset)
    }

    pub fn draw(
        &mut self,
        canvas: &super::render::Canvas,
        engine: &mut super::text::TextEngine,
        theme: &super::theme::Theme,
        field: D2D_RECT_F,
    ) -> Result<()> {
        self.field = field;
        let layout = engine.layout(&self.text, super::address::EDIT, 65536.0)?;
        let (x, _, height) = layout.caret(self.active)?;
        let width = field.right - field.left;
        if x - self.offset < 0.0 {
            self.offset = x;
        }
        if x - self.offset > width - 2.0 {
            self.offset = (x - width + 2.0).ceil();
        }
        self.offset = self
            .offset
            .clamp(0.0, (layout.advance + 2.0 - width).ceil().max(0.0));
        let top = ((field.top + field.bottom - layout.height) / 2.0).floor();
        let origin = (field.left - self.offset, top);
        self.caret = (self.focused && self.start == self.end).then_some(rect(
            (origin.0 + x).round(),
            top,
            1.0,
            height,
        ));
        unsafe {
            canvas
                .context
                .PushAxisAlignedClip(&field, D2D1_ANTIALIAS_MODE_ALIASED);
        }
        let result = (|| {
            let selection = if self.focused && self.start != self.end {
                layout.selection(self.start, self.end - self.start)?
            } else {
                Vec::new()
            };
            let blue = D2D1_COLOR_F {
                r: 0.0,
                g: 120.0 / 255.0,
                b: 212.0 / 255.0,
                a: 1.0,
            };
            let all_selected = !selection.is_empty()
                && self.start == 0
                && self.end == self.text.encode_utf16().count() as u32;
            if !all_selected {
                engine.draw(
                    canvas,
                    &self.text,
                    super::address::EDIT,
                    origin,
                    &theme.text,
                )?;
            }
            for metric in &selection {
                let left = (origin.0 + metric.left).floor();
                let right = origin.0 + metric.left + metric.width;
                let right = if self.offset > 0.0 {
                    right.round()
                } else {
                    right.ceil()
                };
                let selection = rect(
                    left,
                    (origin.1 + metric.top).floor(),
                    (right - left).max(0.0),
                    metric.height.ceil(),
                );
                unsafe {
                    canvas
                        .context
                        .PushAxisAlignedClip(&selection, D2D1_ANTIALIAS_MODE_ALIASED);
                }
                super::details::fill(canvas, selection, &blue);
                let drawn = engine.draw(
                    canvas,
                    &self.text,
                    super::address::EDIT,
                    origin,
                    &theme.edit_selection_text,
                );
                unsafe {
                    canvas.context.PopAxisAlignedClip();
                }
                drawn?;
            }
            Ok(())
        })();
        unsafe {
            canvas.context.PopAxisAlignedClip();
        }
        result
    }
}

#[derive(Clone, Copy)]
pub enum LabelLayout {
    Line,
    Paragraph,
}

pub enum Presentation {
    Hidden,
    Label {
        bounds: (i32, i32, i32, i32),
        layout: LabelLayout,
    },
}

unsafe fn create(parent: HWND, id: usize, font: HFONT, presentation: Presentation) -> Result<HWND> {
    let (style, bounds) = match presentation {
        Presentation::Hidden => (
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            (0, 0, 1, 1),
        ),
        Presentation::Label { bounds, layout } => (
            WS_CHILD
                | WINDOW_STYLE(if matches!(layout, LabelLayout::Paragraph) {
                    ES_MULTILINE | ES_CENTER | ES_AUTOVSCROLL
                } else {
                    ES_AUTOHSCROLL
                } as u32),
            bounds,
        ),
    };
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("EDIT"),
        w!(""),
        style,
        bounds.0,
        bounds.1,
        bounds.2,
        bounds.3,
        Some(parent),
        Some(HMENU(id as _)),
        None,
        None,
    )?;
    let result = (|| {
        SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
        {
            let region = CreateRectRgn(0, 0, 0, 0);
            if region.is_invalid() {
                return Err(windows::core::Error::from_win32());
            }
            if SetWindowRgn(hwnd, Some(region), false) == 0 {
                if !DeleteObject(region.into()).as_bool() {
                    eprintln!("edit region rollback failed");
                }
                return Err(windows::core::Error::from_win32());
            }
        }
        if !SetWindowSubclass(hwnd, Some(procedure), 1, parent.0 as usize).as_bool() {
            return Err(windows::core::Error::from_win32());
        }
        Ok(hwnd)
    })();
    if result.is_err() {
        DestroyWindow(hwnd)?;
    }
    result
}

pub struct Label {
    pub hwnd: HWND,
    font: HFONT,
}

impl Label {
    pub fn new(
        parent: HWND,
        bounds: (i32, i32, i32, i32),
        layout: LabelLayout,
        folder: &str,
    ) -> Result<Self> {
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(parent) } as f32 / 96.0;
        let mut descriptor = LOGFONTW {
            lfHeight: -(12.0 * scale).round() as i32,
            lfWeight: 400,
            lfQuality: CLEARTYPE_QUALITY,
            ..Default::default()
        };
        let family = crate::shell::wide("Segoe UI");
        descriptor.lfFaceName[..family.len()].copy_from_slice(&family);
        unsafe {
            let font = CreateFontIndirectW(&descriptor);
            if font.is_invalid() {
                return Err(windows::core::Error::from_win32());
            }
            let hwnd = match create(parent, 103, font, Presentation::Label { bounds, layout }) {
                Ok(hwnd) => hwnd,
                Err(error) => {
                    if !DeleteObject(font.into()).as_bool() {
                        eprintln!("edit font rollback failed");
                    }
                    return Err(error);
                }
            };
            let control = Self { hwnd, font };
            let shell = crate::shell::Pidl::parse(folder)
                .and_then(|value| value.folder())
                .map_err(|error| {
                    windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error)
                })?;
            windows::Win32::UI::Shell::SHLimitInputEdit(hwnd, &shell)?;
            SendMessageW(
                hwnd,
                windows::Win32::UI::Controls::EM_SETMARGINS,
                Some(WPARAM(3)),
                Some(LPARAM(3 | (3 << 16))),
            );
            Ok(control)
        }
    }
}

pub fn raster_label(hwnd: HWND, width: i32, height: i32) -> Result<crate::shell::icons::Rgba> {
    let dib = super::dib::Dib::new(width, height)?;
    unsafe {
        let pixels = std::slice::from_raw_parts_mut(dib.pixels, (width * height * 4) as usize);
        pixels.fill(if crate::shell::theme::dark() { 33 } else { 255 });
        let saved = SaveDC(dib.dc);
        if saved == 0 {
            return Err(windows::core::Error::from_win32());
        }
        if IntersectClipRect(dib.dc, 1, 2, width - 1, height - 2).0 == 0 {
            return Err(windows::core::Error::from_win32());
        }
        SetViewportOrgEx(dib.dc, 1, 2, None).ok()?;
        SendMessageW(
            hwnd,
            WM_PRINTCLIENT,
            Some(WPARAM(dib.dc.0 as usize)),
            Some(LPARAM(PRF_CLIENT as isize | PRF_ERASEBKGND as isize)),
        );
        RestoreDC(dib.dc, saved).ok()?;
        let border = windows::Win32::Foundation::COLORREF(if crate::shell::theme::dark() {
            0xffffff
        } else {
            0
        });
        SetDCBrushColor(dib.dc, border);
        if FrameRect(
            dib.dc,
            &windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            },
            HBRUSH(GetStockObject(DC_BRUSH).0),
        ) == 0
        {
            return Err(windows::core::Error::from_win32());
        }
        GdiFlush().ok()?;
        let rgba = pixels
            .chunks_exact(4)
            .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], 255])
            .collect();
        Ok(crate::shell::icons::Rgba {
            width: width as u32,
            height: height as u32,
            pixels: rgba,
        })
    }
}

impl Drop for Label {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(Some(self.hwnd)).as_bool() {
                if let Err(error) = DestroyWindow(self.hwnd) {
                    eprintln!("edit label release: {error}");
                }
            }
            if !DeleteObject(self.font.into()).as_bool() {
                eprintln!("edit label font release failed");
            }
        }
    }
}

pub struct Controls {
    pub address: HWND,
    pub search: HWND,
    font: HFONT,
    dpi: u32,
}

impl Controls {
    pub fn new(parent: HWND) -> Result<Self> {
        unsafe {
            let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(parent);
            let mut font = LOGFONTW {
                lfHeight: -(14.0 * dpi as f32 / 96.0).round() as i32,
                ..Default::default()
            };
            let family = crate::shell::wide("Segoe UI Variable Small");
            font.lfFaceName[..family.len()].copy_from_slice(&family);
            let font = CreateFontIndirectW(&font);
            if font.is_invalid() {
                return Err(windows::core::Error::from_win32());
            }
            let address = match create(parent, 101, font, Presentation::Hidden) {
                Ok(hwnd) => hwnd,
                Err(error) => {
                    let _ = DeleteObject(font.into());
                    return Err(error);
                }
            };
            let search = match create(parent, 102, font, Presentation::Hidden) {
                Ok(hwnd) => hwnd,
                Err(error) => {
                    DestroyWindow(address)?;
                    let _ = DeleteObject(font.into());
                    return Err(error);
                }
            };
            Ok(Self {
                address,
                search,
                font,
                dpi,
            })
        }
    }

    pub fn refresh(&mut self, dpi: u32) -> Result<()> {
        if self.dpi == dpi {
            return Ok(());
        }
        let mut description = LOGFONTW {
            lfHeight: -(14.0 * dpi as f32 / 96.0).round() as i32,
            ..Default::default()
        };
        let family = crate::shell::wide("Segoe UI Variable Small");
        description.lfFaceName[..family.len()].copy_from_slice(&family);
        unsafe {
            let font = CreateFontIndirectW(&description);
            if font.is_invalid() {
                return Err(windows::core::Error::from_win32());
            }
            for control in [self.address, self.search] {
                SendMessageW(control, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
            }
            let previous = std::mem::replace(&mut self.font, font);
            self.dpi = dpi;
            if !DeleteObject(previous.into()).as_bool() {
                return Err(windows::core::Error::from_win32());
            }
        }
        Ok(())
    }
}

impl Drop for Controls {
    fn drop(&mut self) {
        unsafe {
            for hwnd in [self.address, self.search] {
                if IsWindow(Some(hwnd)).as_bool() {
                    if let Err(error) = DestroyWindow(hwnd) {
                        eprintln!("edit release: {error}");
                    }
                }
            }
            if !DeleteObject(self.font.into()).as_bool() {
                eprintln!("edit font release failed");
            }
        }
    }
}

pub fn set_text(hwnd: HWND, text: &str, selection: (u32, u32)) -> Result<()> {
    let wide = crate::shell::wide(text);
    unsafe {
        SetWindowTextW(hwnd, PCWSTR(wide.as_ptr()))?;
        SendMessageW(
            hwnd,
            EM_SETSEL,
            Some(WPARAM(selection.0 as usize)),
            Some(LPARAM(selection.1 as isize)),
        );
    }
    Ok(())
}

pub fn focus(hwnd: HWND) {
    unsafe {
        let _ = SetFocus(Some(hwnd));
    }
}

pub fn snapshot(hwnd: HWND) -> Snapshot {
    unsafe {
        let mut buffer = vec![0u16; GetWindowTextLengthW(hwnd).max(0) as usize + 1];
        let length = GetWindowTextW(hwnd, &mut buffer).max(0) as usize;
        let (mut start, mut end) = (0u32, 0u32);
        SendMessageW(
            hwnd,
            EM_GETSEL,
            Some(WPARAM(&mut start as *mut u32 as usize)),
            Some(LPARAM(&mut end as *mut u32 as isize)),
        );
        let mut point = POINT::default();
        let active = if GetCaretPos(&mut point).is_ok() {
            let position = SendMessageW(
                hwnd,
                0xd7,
                Some(WPARAM(0)),
                Some(LPARAM(
                    ((point.y as u32) << 16 | point.x as u32 & 0xffff) as isize,
                )),
            )
            .0 as u32
                & 0xffff;
            if position.abs_diff(start) < position.abs_diff(end) {
                start
            } else {
                end
            }
        } else {
            end
        };
        Snapshot {
            text: String::from_utf16_lossy(&buffer[..length]),
            start,
            end,
            active,
            focused: GetFocus() == hwnd,
            kind: Kind::None,
        }
    }
}

fn changed(parent: HWND, hwnd: HWND, snapshot: Snapshot) {
    if unsafe { GetDlgCtrlID(hwnd) } == 103 {
        super::rename::changed(snapshot);
    } else {
        super::address_input::edit_changed(parent, hwnd, snapshot);
    }
}

pub fn notify(parent: HWND, hwnd: HWND) {
    let mut snapshot = snapshot(hwnd);
    snapshot.kind = Kind::Movement;
    changed(parent, hwnd, snapshot);
}

pub fn notify_preview(parent: HWND, hwnd: HWND) {
    let mut snapshot = snapshot(hwnd);
    snapshot.kind = Kind::Programmatic;
    super::WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            if hwnd == window.editors.search {
                window.address.search_edit.update(snapshot);
            } else if hwnd == window.editors.address {
                window.address.address_edit.update(snapshot);
            }
        }
    });
    super::address_input::invalidate(parent);
}

fn history(parent: HWND, control: HWND, key: u32) {
    let record = if super::rename::is_control(control) {
        super::rename::take_history(key)
    } else {
        super::WINDOW.with(|slot| {
            slot.borrow_mut().as_mut().and_then(|window| {
                let state = if control == window.editors.address {
                    &mut window.address.address_edit
                } else {
                    &mut window.address.search_edit
                };
                state.take_history(key)
            })
        })
    };
    let Some(record) = record else {
        return;
    };
    let selection = if record.active == record.start {
        (record.end, record.start)
    } else {
        (record.start, record.end)
    };
    if let Err(error) = set_text(control, &record.text, selection) {
        super::fail(error);
        return;
    }
    let mut snapshot = snapshot(control);
    snapshot.kind = Kind::History;
    changed(parent, control, snapshot);
}

pub fn place(parent: HWND) -> Result<()> {
    let fields = super::WINDOW.with(|slot| {
        slot.borrow().as_ref().map(|window| {
            [
                (window.editors.address, window.address.address_edit.field),
                (window.editors.search, window.address.search_edit.field),
            ]
        })
    });
    if let Some(fields) = fields {
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(parent) } as f32 / 96.0;
        for (control, field) in fields {
            let expected = (
                (field.left * scale).round() as i32,
                ((field.top + 40.0) * scale).round() as i32,
                ((field.right - field.left) * scale).round().max(1.0) as i32,
                ((field.bottom - field.top) * scale).round().max(1.0) as i32,
            );
            unsafe {
                let mut actual = windows::Win32::Foundation::RECT::default();
                GetWindowRect(control, &mut actual)?;
                let origin = crate::frame::client_to_screen(parent, expected.0, expected.1);
                if (
                    actual.left,
                    actual.top,
                    actual.right - actual.left,
                    actual.bottom - actual.top,
                ) != (origin.0, origin.1, expected.2, expected.3)
                {
                    SetWindowPos(
                        control,
                        None,
                        expected.0,
                        expected.1,
                        expected.2,
                        expected.3,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    )?;
                }
            }
        }
    }
    let caret = super::WINDOW.with(|slot| {
        slot.borrow().as_ref().and_then(|window| {
            let edit = if window.address.editing {
                &window.address.address_edit
            } else {
                &window.address.search_edit
            };
            let control = if window.address.editing {
                window.editors.address
            } else {
                window.editors.search
            };
            Some((control, edit.caret?, edit.field))
        })
    });
    if let Some((control, caret, field)) = caret {
        if !control.is_invalid()
            && unsafe { GetFocus() } == control
            && unsafe { GetForegroundWindow() } == parent
        {
            let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(parent) } as f32 / 96.0;
            unsafe {
                let mut info = GUITHREADINFO {
                    cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                    ..Default::default()
                };
                GetGUIThreadInfo(
                    windows::Win32::System::Threading::GetCurrentThreadId(),
                    &mut info,
                )?;
                if info.hwndCaret != control {
                    return Ok(());
                }
                SetCaretPos(
                    ((caret.left - field.left) * scale).round() as i32,
                    ((caret.top - field.top) * scale).round() as i32,
                )?;
            }
        }
    }
    Ok(())
}

pub fn composing(hwnd: HWND) -> bool {
    unsafe {
        use windows::Win32::UI::Input::Ime::*;
        let context = ImmGetContext(hwnd);
        if context.is_invalid() {
            return false;
        }
        let length = ImmGetCompositionStringW(context, GCS_COMPSTR, None, 0);
        let released = ImmReleaseContext(hwnd, context);
        if !released.as_bool() {
            eprintln!("edit IME context release failed");
        }
        length > 0
    }
}

unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    parent: usize,
) -> LRESULT {
    let parent = HWND(parent as _);
    let _context = super::context::enter(parent);
    if let Some(result) = super::rename::edit_message(hwnd, message, wparam, lparam) {
        return result;
    }
    let label = GetDlgCtrlID(hwnd) == 103;
    if message == WM_UNDO || message == windows::Win32::UI::Controls::EM_UNDO {
        history(parent, hwnd, 0x5a);
        return LRESULT(1);
    }
    if message == windows::Win32::UI::Controls::EM_CANUNDO {
        return LRESULT(super::WINDOW.with(|slot| {
            slot.borrow().as_ref().is_some_and(|window| {
                if super::rename::is_control(hwnd) {
                    return super::rename::can_undo();
                }
                let edit = if hwnd == window.editors.address {
                    &window.address.address_edit
                } else {
                    &window.address.search_edit
                };
                !edit.undo.is_empty()
            })
        }) as isize);
    }
    if matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN) {
        if !label && super::tabs_input::key(parent, wparam.0 as u32) {
            return LRESULT(0);
        }
        if matches!(wparam.0, 0x59 | 0x5a)
            && super::input::modifiers().ctrl
            && windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) >= 0
            && !composing(hwnd)
        {
            history(parent, hwnd, wparam.0 as u32);
            return LRESULT(0);
        }
        if !label && super::address_input::edit_key(parent, hwnd, wparam.0 as u32) {
            return LRESULT(0);
        }
        if wparam.0 == 0x41
            && super::input::modifiers().ctrl
            && windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) >= 0
        {
            SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
            changed(parent, hwnd, snapshot(hwnd));
            return LRESULT(0);
        }
        if matches!(wparam.0, 0x08 | 0x2e)
            && super::input::modifiers().ctrl
            && windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) >= 0
            && !composing(hwnd)
        {
            let before = snapshot(hwnd);
            if before.start == before.end {
                let direction = if wparam.0 == 0x08 { 0x25 } else { 0x27 };
                DefSubclassProc(hwnd, WM_KEYDOWN, WPARAM(direction), LPARAM(1));
                let after = snapshot(hwnd);
                let mut boundary = if direction == 0x25 {
                    after.start
                } else {
                    after.end
                };
                if direction == 0x25 {
                    let mut offset = 0u32;
                    for character in before.text.chars() {
                        if offset >= boundary
                            && offset < before.start
                            && (character.is_alphanumeric() || character == '_')
                        {
                            boundary = offset;
                            break;
                        }
                        offset += character.len_utf16() as u32;
                    }
                }
                SendMessageW(
                    hwnd,
                    EM_SETSEL,
                    Some(WPARAM(before.start as usize)),
                    Some(LPARAM(boundary as isize)),
                );
            }
            SendMessageW(
                hwnd,
                windows::Win32::UI::Controls::EM_REPLACESEL,
                Some(WPARAM(1)),
                Some(LPARAM(w!("").0 as isize)),
            );
            let mut snapshot = snapshot(hwnd);
            snapshot.kind = Kind::Replace;
            changed(parent, hwnd, snapshot);
            return LRESULT(0);
        }
    }
    if message == WM_CHAR && matches!(wparam.0, 9 | 13 | 27) {
        return LRESULT(0);
    }
    if message == WM_CHAR && matches!(wparam.0, 0x7f | 0x19 | 0x1a) {
        return LRESULT(0);
    }
    let result = DefSubclassProc(hwnd, message, wparam, lparam);
    if message == WM_SETFOCUS {
        if let Err(error) = HideCaret(Some(hwnd)) {
            eprintln!("edit native caret: {error}");
        }
    }
    if matches!(
        message,
        WM_CHAR
            | WM_KEYDOWN
            | WM_SYSKEYDOWN
            | WM_SETFOCUS
            | WM_KILLFOCUS
            | WM_CUT
            | WM_COPY
            | WM_PASTE
            | WM_CLEAR
            | WM_IME_COMPOSITION
            | WM_IME_ENDCOMPOSITION
            | WM_MOUSEMOVE
            | WM_LBUTTONUP
    ) {
        let mut snapshot = snapshot(hwnd);
        snapshot.kind = match message {
            WM_CHAR if wparam.0 == 8 => Kind::Backward,
            WM_CHAR => Kind::Typing,
            WM_KEYDOWN if wparam.0 == 0x2e => Kind::Forward,
            WM_KEYDOWN if matches!(wparam.0, 0x25 | 0x27 | 0x24 | 0x23) => Kind::Movement,
            WM_CUT | WM_PASTE | WM_CLEAR => Kind::Replace,
            WM_IME_COMPOSITION => Kind::Composition,
            WM_SETFOCUS | WM_KILLFOCUS | WM_IME_ENDCOMPOSITION => Kind::Movement,
            _ => Kind::None,
        };
        changed(parent, hwnd, snapshot);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(state: &mut State, text: &str, kind: Kind) {
        let end = text.encode_utf16().count() as u32;
        state.update(Snapshot {
            text: text.into(),
            start: end,
            end,
            active: end,
            focused: true,
            kind,
        });
    }

    fn history(state: &mut State, key: u32) -> String {
        let record = state.take_history(key).expect("history frame");
        let text = record.text.clone();
        state.update(Snapshot {
            text: record.text,
            start: record.start,
            end: record.end,
            active: record.active,
            focused: true,
            kind: Kind::History,
        });
        text
    }

    #[test]
    fn cut_and_paste_are_separate_reversible_edits() {
        let mut state = State::default();
        update(&mut state, "dummy", Kind::Programmatic);
        update(&mut state, "", Kind::Replace);
        update(&mut state, "dummy", Kind::Replace);
        assert_eq!(history(&mut state, 0x5a), "");
        assert_eq!(history(&mut state, 0x5a), "dummy");
        assert_eq!(history(&mut state, 0x59), "");
        assert_eq!(history(&mut state, 0x59), "dummy");
    }

    #[test]
    fn caret_movement_splits_typing_and_new_edit_discards_redo() {
        let mut state = State::default();
        update(&mut state, "a", Kind::Typing);
        update(&mut state, "ab", Kind::Typing);
        assert_eq!(state.undo.len(), 1);
        update(&mut state, "ab", Kind::Movement);
        update(&mut state, "abc", Kind::Typing);
        assert_eq!(history(&mut state, 0x5a), "ab");
        update(&mut state, "abx", Kind::Typing);
        assert!(state.redo.is_empty());
        assert_eq!(history(&mut state, 0x5a), "ab");
        assert_eq!(history(&mut state, 0x5a), "");
    }
}
