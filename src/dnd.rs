use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows::core::{implement, w, Ref, Result};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, POINTL, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    CoCreateInstance, IDataObject, CLSCTX_INPROC_SERVER, DVASPECT_CONTENT, FORMATETC, STGMEDIUM,
    STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT,
};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, OleInitialize, OleUninitialize, RegisterDragDrop,
    ReleaseStgMedium, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE,
    DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::Shell::{
    CLSID_DragDropHelper, IDropTargetHelper, SHDoDragDrop, DROPDESCRIPTION, DROPIMAGE_INVALID,
    DROPIMAGE_NOIMAGE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    PostThreadMessageW, ShowWindow, ShowWindowAsync, SW_HIDE, SW_SHOWNA, WM_KEYDOWN,
};

use crate::shell::ops;

/// What a drop lands on, to light it up: a row of the view, a row of the navigation pane
/// or a crumb of the address bar.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Spot {
    Row(usize),
    Nav(usize),
    Crumb(usize),
    Pin {
        at: usize,
        line: i32,
        row: Option<usize>,
    },
}

/// Where a drop at a client point (physical pixels) would land.
#[derive(Clone, PartialEq, Debug)]
pub struct Target {
    /// The folder shown in the view.
    pub folder: String,
    /// The item under the pointer (a folder row, a tree row, a crumb), if any.
    pub item: Option<String>,
    pub spot: Option<Spot>,
}

fn same_folder_move(
    target: &Target,
    source_parent: Option<&str>,
    keys: MODIFIERKEYS_FLAGS,
) -> bool {
    target.item.is_none()
        && target.spot.is_none()
        && keys.0
            & (windows::Win32::System::SystemServices::MK_CONTROL.0
                | windows::Win32::System::Ole::MK_ALT)
            == 0
        && source_parent.is_some_and(|parent| crate::app::ids::same(parent, &target.folder))
}

pub trait Resolver {
    fn target_at(&self, client: POINT) -> Option<Target>;
    fn highlight(&self, spot: Option<Spot>);
    /// Over a crumb the address bar draws the drag's caption itself, as Explorer's XAML bar
    /// does: its text ("Mover para ..."), the effect and the pointer (client pixels).
    fn caption(&self, caption: Option<(String, DROPEFFECT, POINT)>);
    /// A drag came in, carrying these files (the bar draws the first one's picture).
    fn entered(&self, files: Vec<String>);
    /// A tab only activates on hover; it rejects the drop and uses the bar's preview.
    fn tab_preview(&self, client: POINT) -> bool;
    fn left(&self);
    /// The drop is done; for files dropped into a folder, what they became.
    fn dropped(&self, transfer: Option<ops::Transfer>);
}

#[derive(Clone)]
struct Current {
    target: Target,
    drop: IDropTarget,
}

#[implement(IDropTarget)]
struct WindowDropTarget {
    hwnd: HWND,
    resolver: Rc<dyn Resolver>,
    helper: Option<IDropTargetHelper>,
    data: RefCell<Option<IDataObject>>,
    source_parent: RefCell<Option<String>>,
    same_folder: std::cell::Cell<bool>,
    current: RefCell<Option<Current>>,
    rejected: RefCell<Option<Target>>,
    caption: RefCell<String>,
    image_hidden: std::cell::Cell<bool>,
}

impl WindowDropTarget {
    fn client(&self, pt: &POINTL) -> POINT {
        let mut p = POINT { x: pt.x, y: pt.y };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut p);
        }
        p
    }

    fn retarget(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) {
        let source_parent = self.source_parent.borrow().clone();
        let target = self.resolver.target_at(self.client(pt));
        let same_folder = target
            .as_ref()
            .is_some_and(|target| same_folder_move(target, source_parent.as_deref(), keys));
        self.same_folder.set(same_folder);
        let target = target.filter(|_| !same_folder);
        let old = self.current.borrow().clone();
        let rejected = self.rejected.borrow().clone();
        if rejected.as_ref() != target.as_ref() {
            self.rejected.borrow_mut().take();
        }
        if old.as_ref().map(|current| &current.target) == target.as_ref()
            || (rejected.is_some() && rejected.as_ref() == target.as_ref())
        {
            unsafe {
                match old {
                    Some(current) => {
                        if let Err(error) = current.drop.DragOver(keys, *pt, effect) {
                            eprintln!("drag over: {error}");
                            *effect = DROPEFFECT_NONE;
                        }
                    }
                    None => *effect = DROPEFFECT_NONE,
                }
            }
            return;
        }
        self.current.borrow_mut().take();
        if let Some(old) = old {
            if let Err(error) = unsafe { old.drop.DragLeave() } {
                eprintln!("drag retarget leave: {error}");
            }
        }
        self.resolver
            .highlight(target.as_ref().and_then(|target| target.spot));
        let Some(target) = target else {
            unsafe { *effect = DROPEFFECT_NONE };
            return;
        };
        let data = self.data.borrow().clone();
        let drop = match target.spot {
            Some(Spot::Pin { at, .. }) => Ok(crate::shell::quick_access::drop_target(at)),
            _ => ops::drop_target(self.hwnd, &target.folder, target.item.as_deref()),
        };
        match drop {
            Ok(drop) => {
                if let Err(error) = unsafe { drop.DragEnter(data.as_ref(), keys, *pt, effect) } {
                    eprintln!("drag target enter: {error}");
                    unsafe { *effect = DROPEFFECT_NONE };
                }
                *self.current.borrow_mut() = Some(Current { target, drop });
            }
            Err(error) => {
                *self.rejected.borrow_mut() = Some(target);
                eprintln!("drag target: {error}");
                unsafe { *effect = DROPEFFECT_NONE };
            }
        }
    }
}

impl WindowDropTarget {
    fn update_caption(&self, pt: &POINTL, effect: DROPEFFECT) {
        let crumb = matches!(
            self.current.borrow().as_ref().and_then(|c| c.target.spot),
            Some(Spot::Crumb(_))
        );
        let at = self.client(pt);
        let tab = self.resolver.tab_preview(at);
        let data = self.data.borrow().clone();
        if self.same_folder.get() {
            if let Some(data) = &data {
                if let Err(error) = clear_description(data) {
                    eprintln!("clear drag caption: {error}");
                }
            }
        }
        let caption = if tab {
            Some((String::new(), DROPEFFECT_NONE, at))
        } else {
            match data {
                Some(data) if crumb => {
                    if let Some(text) = take_description(&data) {
                        *self.caption.borrow_mut() = text;
                    }
                    let text = self.caption.borrow().clone();
                    (effect != DROPEFFECT_NONE && !text.is_empty())
                        .then(|| (text, effect, self.client(pt)))
                }
                _ => None,
            }
        };
        let hide = caption.is_some();
        if !hide {
            self.show_image();
        }
        self.image_hidden.set(hide);
        self.resolver.caption(caption);
    }
}

/// The drag image's window, which the drag image manager keeps in the data object.
fn drag_window(data: &IDataObject) -> Option<HWND> {
    unsafe {
        let format = RegisterClipboardFormatW(w!("DragWindow")) as u16;
        let fmt = FORMATETC {
            cfFormat: format,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let mut medium = data.GetData(&fmt).ok()?;
        let global = medium.u.hGlobal;
        let ptr = GlobalLock(global) as *const u32;
        let window =
            (!ptr.is_null()).then(|| HWND(std::ptr::read_unaligned(ptr) as usize as *mut _));
        let _ = GlobalUnlock(global);
        ReleaseStgMedium(&mut medium);
        window.filter(|w| !w.is_invalid())
    }
}

impl WindowDropTarget {
    fn hide_image(&self) {
        if self.image_hidden.get() {
            let data = self.data.borrow().clone();
            let window = data.as_ref().and_then(drag_window);
            if let Some(window) = window {
                unsafe {
                    let _ = ShowWindow(window, SW_HIDE);
                }
            }
        }
    }

    fn show_image(&self) {
        if self.image_hidden.replace(false) {
            let data = self.data.borrow().clone();
            let window = data.as_ref().and_then(drag_window);
            if let Some(window) = window {
                unsafe {
                    let _ = ShowWindowAsync(window, SW_SHOWNA);
                }
            }
        }
    }
}

fn take_description(data: &IDataObject) -> Option<String> {
    unsafe {
        let format = RegisterClipboardFormatW(w!("DropDescription")) as u16;
        let fmt = FORMATETC {
            cfFormat: format,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let mut medium = data.GetData(&fmt).ok()?;
        let global = medium.u.hGlobal;
        let ptr = GlobalLock(global) as *const DROPDESCRIPTION;
        let description = (!ptr.is_null()).then(|| std::ptr::read_unaligned(ptr));
        let _ = GlobalUnlock(global);
        ReleaseStgMedium(&mut medium);
        let description = description?;
        let (kind, message, insert) = (
            description.r#type,
            description.szMessage,
            description.szInsert,
        );
        if kind == DROPIMAGE_NOIMAGE || kind == DROPIMAGE_INVALID {
            return None;
        }
        let text = |w: &[u16]| {
            String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())])
        };
        let caption = text(&message).replace("%1", &text(&insert));

        if let Err(error) = clear_description(data) {
            eprintln!("take drag caption: {error}");
        }
        (!caption.is_empty()).then_some(caption)
    }
}

fn clear_description(data: &IDataObject) -> Result<()> {
    unsafe {
        let fmt = FORMATETC {
            cfFormat: RegisterClipboardFormatW(w!("DropDescription")) as u16,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let global = GlobalAlloc(
            GMEM_MOVEABLE | GMEM_ZEROINIT,
            std::mem::size_of::<DROPDESCRIPTION>(),
        )?;
        let ptr = GlobalLock(global) as *mut DROPDESCRIPTION;
        if ptr.is_null() {
            let error = windows::core::Error::from_win32();
            let _ = windows::Win32::Foundation::GlobalFree(Some(global));
            return Err(error);
        }
        std::ptr::addr_of_mut!((*ptr).r#type).write_unaligned(DROPIMAGE_NOIMAGE);
        let _ = GlobalUnlock(global);
        let mut medium = STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: global },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        };
        let result = data.SetData(&fmt, &medium, true);
        if result.is_err() {
            ReleaseStgMedium(&mut medium);
        }
        result
    }
}

impl IDropTarget_Impl for WindowDropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<'_, IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        *self.data.borrow_mut() = data.cloned();
        self.image_hidden.set(false);
        self.same_folder.set(false);
        self.caption.borrow_mut().clear();
        let files = data.as_ref().map(ops::drag_items).unwrap_or_default();
        let parent = files
            .first()
            .and_then(|path| std::path::Path::new(path).parent())
            .map(|parent| parent.to_string_lossy().into_owned());
        let parent = parent.filter(|parent| {
            files.iter().all(|path| {
                std::path::Path::new(path)
                    .parent()
                    .is_some_and(|candidate| {
                        crate::app::ids::same(&candidate.to_string_lossy(), parent)
                    })
            })
        });
        *self.source_parent.borrow_mut() = parent;
        if DRAG_THREAD.load(Ordering::SeqCst) == 0
            && files.len() == 1
            && std::path::Path::new(&files[0]).is_dir()
        {
            if let Some(data) = data.as_ref() {
                if let Err(error) = initialize_drag_picture(data, &files[0]) {
                    eprintln!("incoming drag picture: {error}");
                }
            }
        }
        self.resolver.entered(files);
        self.retarget(keys, pt, effect);
        self.update_caption(pt, unsafe { *effect });
        if let Some(helper) = &self.helper {
            let p = POINT { x: pt.x, y: pt.y };
            unsafe {
                let _ = helper.DragEnter(self.hwnd, data.as_ref(), &p, *effect);
            }
        }
        self.hide_image();
        Ok(())
    }

    fn DragOver(
        &self,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        self.retarget(keys, pt, effect);
        self.update_caption(pt, unsafe { *effect });
        if let Some(helper) = &self.helper {
            let p = POINT { x: pt.x, y: pt.y };
            unsafe {
                let _ = helper.DragOver(&p, *effect);
            }
        }
        self.hide_image();
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        let old = self.current.borrow_mut().take();
        if let Some(old) = old {
            unsafe {
                if let Err(error) = old.drop.DragLeave() {
                    eprintln!("drop target leave: {error}");
                }
            }
        }
        self.show_image();
        self.same_folder.set(false);
        *self.data.borrow_mut() = None;
        self.rejected.borrow_mut().take();
        *self.source_parent.borrow_mut() = None;
        self.resolver.highlight(None);
        self.resolver.caption(None);
        self.resolver.left();
        if let Some(helper) = &self.helper {
            unsafe {
                if let Err(error) = helper.DragLeave() {
                    eprintln!("drop helper leave: {error}");
                }
            }
        }
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<'_, IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        self.retarget(keys, pt, effect);
        if let Some(helper) = &self.helper {
            let p = POINT { x: pt.x, y: pt.y };
            unsafe {
                let _ = helper.Drop(data.as_ref(), &p, *effect);
            }
        }
        let current = self.current.borrow_mut().take();
        let mut transfer = None;
        if let Some(c) = current {
            let dest = c
                .target
                .item
                .clone()
                .unwrap_or_else(|| c.target.folder.clone());
            let sources = data.as_ref().map(ops::dropped_files).unwrap_or_default();
            let before = (!sources.is_empty() && std::path::Path::new(&dest).is_dir())
                .then(|| ops::snapshot(&dest))
                .flatten();
            if before.is_some() {
                if let Some(d) = data.as_ref() {
                    ops::transfer_now(d);
                }
            }
            unsafe {
                if c.drop.Drop(data.as_ref(), keys, *pt, effect).is_err() {
                    *effect = DROPEFFECT_NONE;
                }
            }
            transfer = before.map(|before| ops::transfer(&sources, &dest, &before));
        }
        self.show_image();
        self.same_folder.set(false);
        *self.data.borrow_mut() = None;
        self.rejected.borrow_mut().take();
        *self.source_parent.borrow_mut() = None;
        self.resolver.highlight(None);
        self.resolver.caption(None);
        self.resolver.dropped(transfer);
        Ok(())
    }
}


pub fn register(hwnd: HWND, resolver: Box<dyn Resolver>) -> Result<()> {
    let helper: Option<IDropTargetHelper> =
        match unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) } {
            Ok(helper) => Some(helper),
            Err(error) => {
                eprintln!("drop helper: {error}");
                None
            }
        };
    let target: IDropTarget = WindowDropTarget {
        hwnd,
        resolver: Rc::from(resolver),
        helper,
        data: RefCell::new(None),
        source_parent: RefCell::new(None),
        same_folder: std::cell::Cell::new(false),
        current: RefCell::new(None),
        rejected: RefCell::new(None),
        caption: RefCell::new(String::new()),
        image_hidden: std::cell::Cell::new(false),
    }
    .into();
    unsafe { RegisterDragDrop(hwnd, &target) }
}

pub fn drag_apart(
    hwnd: HWND,
    paths: Vec<String>,
    done: impl FnOnce(DROPEFFECT) + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    if DRAG_THREAD
        .compare_exchange(0, STARTING, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "A shell drag is already active",
        ));
    }
    DRAG_CANCELLED.store(false, Ordering::SeqCst);
    let ui_thread = unsafe { GetCurrentThreadId() };
    let window = hwnd.0 as isize;
    let worker = std::thread::Builder::new()
        .name("shell-drag".into())
        .spawn(move || unsafe {
            if let Err(error) = OleInitialize(None) {
                eprintln!("drag OLE initialization: {error}");
                DRAG_THREAD.store(0, Ordering::SeqCst);
                done(DROPEFFECT_NONE);
                return;
            }
            let me = GetCurrentThreadId();
            let mut message = windows::Win32::UI::WindowsAndMessaging::MSG::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::PeekMessageW(
                &mut message,
                None,
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::PM_NOREMOVE,
            );
            DRAG_THREAD.store(me, Ordering::SeqCst);
            let effect = if AttachThreadInput(me, ui_thread, true).as_bool() {
                let effect = if DRAG_CANCELLED.load(Ordering::SeqCst) {
                    DROPEFFECT_NONE
                } else {
                    drag(HWND(window as *mut _), &paths)
                };
                if !AttachThreadInput(me, ui_thread, false).as_bool() {
                    eprintln!("drag input detach: {}", windows::core::Error::from_win32());
                }
                effect
            } else {
                eprintln!("drag input attach: {}", windows::core::Error::from_win32());
                DROPEFFECT_NONE
            };
            OleUninitialize();
            DRAG_THREAD.store(0, Ordering::SeqCst);
            done(effect);
        });
    if worker.is_err() {
        DRAG_THREAD.store(0, Ordering::SeqCst);
    }
    worker
}

pub fn unregister(hwnd: HWND) -> Result<()> {
    unsafe { windows::Win32::System::Ole::RevokeDragDrop(hwnd) }
}

pub fn active_workers() -> usize {
    usize::from(DRAG_THREAD.load(Ordering::SeqCst) != 0)
}

const STARTING: u32 = u32::MAX;
static DRAG_THREAD: AtomicU32 = AtomicU32::new(0);
static DRAG_CANCELLED: AtomicBool = AtomicBool::new(false);

pub fn cancel_drag() -> bool {
    let thread = DRAG_THREAD.load(Ordering::SeqCst);
    if thread == 0 {
        return false;
    }
    DRAG_CANCELLED.store(true, Ordering::SeqCst);
    if thread != STARTING {
        if let Err(error) = unsafe {
            PostThreadMessageW(thread, WM_KEYDOWN, WPARAM(VK_ESCAPE.0 as usize), LPARAM(0))
        } {
            eprintln!("cancel shell drag: {error}");
        }
    }
    true
}

/// Drags the given items (all in `folder`) with the shell's data object and drag image.
fn drag(hwnd: HWND, paths: &[String]) -> DROPEFFECT {
    let Ok(data) = ops::data_object(hwnd, paths) else {
        return DROPEFFECT_NONE;
    };
    if let Some(path) = paths.first().filter(|_| paths.len() == 1) {
        let _ = initialize_drag_picture(&data, path);
    }
    unsafe {
        SHDoDragDrop(
            Some(hwnd),
            &data,
            None,
            DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK,
        )
        .unwrap_or(DROPEFFECT_NONE)
    }
}

fn initialize_drag_picture(data: &IDataObject, path: &str) -> Result<()> {
    use windows::Win32::Foundation::{COLORREF, SIZE};
    use windows::Win32::Graphics::Gdi::{
        CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::UI::Shell::{
        IDragSourceHelper2, DSH_ALLOWDROPDESCRIPTIONTEXT, SHDRAGIMAGE,
    };
    let picture = crate::shell::icons::shell_picture(path, 96)
        .or_else(|_| crate::shell::icons::drag_picture(path, 96))
        .map_err(|error| windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error))?;
    unsafe {
        set_drag_flag(data, "InShellDragLoop", true)?;
        let helper: IDragSourceHelper2 =
            CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER)?;
        helper.SetFlags(DSH_ALLOWDROPDESCRIPTIONTEXT.0 as u32)?;
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: picture.width as i32,
                biHeight: -(picture.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let output = std::slice::from_raw_parts_mut(bits as *mut u8, picture.pixels.len());
        for (src, dst) in picture
            .pixels
            .chunks_exact(4)
            .zip(output.chunks_exact_mut(4))
        {
            let premul = |channel: u8| ((channel as u16 * src[3] as u16 + 127) / 255) as u8;
            dst.copy_from_slice(&[premul(src[2]), premul(src[1]), premul(src[0]), src[3]]);
        }
        let image = SHDRAGIMAGE {
            sizeDragImage: SIZE {
                cx: picture.width as i32,
                cy: picture.height as i32,
            },
            ptOffset: POINT {
                x: picture.width as i32 / 2,
                y: picture.height as i32 - 7,
            },
            hbmpDragImage: bitmap,
            crColorKey: COLORREF(0xFFFF_FFFF),
        };
        let result = helper.InitializeFromBitmap(&image, data);
        if result.is_err() {
            let _ = DeleteObject(bitmap.into());
        }
        result?;
        set_drag_flag(data, "IsShowingText", true)?;
        set_drag_flag(data, "UsingDefaultDragImage", true)
    }
}

unsafe fn set_drag_flag(data: &IDataObject, name: &str, value: bool) -> Result<()> {
    let name = crate::shell::wide(name);
    let format = RegisterClipboardFormatW(windows::core::PCWSTR(name.as_ptr())) as u16;
    let fmt = FORMATETC {
        cfFormat: format,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let global = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, 4)?;
    let ptr = GlobalLock(global) as *mut u32;
    if ptr.is_null() {
        let error = windows::core::Error::from_win32();
        let _ = windows::Win32::Foundation::GlobalFree(Some(global));
        return Err(error);
    }
    ptr.write(u32::from(value));
    let _ = GlobalUnlock(global);
    let mut medium = STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: global },
        pUnkForRelease: std::mem::ManuallyDrop::new(None),
    };
    let result = data.SetData(&fmt, &medium, true);
    if result.is_err() {
        ReleaseStgMedium(&mut medium);
    }
    result
}
#[cfg(test)]
mod drop_tests {
    use super::*;
    use std::cell::Cell;
    use std::path::PathBuf;

    struct Probe {
        destination: RefCell<Target>,
        transfer: RefCell<Option<ops::Transfer>>,
        entered: Cell<usize>,
        left: Cell<usize>,
    }
    struct Adapter(Rc<Probe>);
    impl Resolver for Adapter {
        fn target_at(&self, _: POINT) -> Option<Target> {
            Some(self.0.destination.borrow().clone())
        }
        fn highlight(&self, _: Option<Spot>) {}
        fn caption(&self, _: Option<(String, DROPEFFECT, POINT)>) {}
        fn entered(&self, files: Vec<String>) {
            self.0.entered.set(files.len());
        }
        fn tab_preview(&self, _: POINT) -> bool {
            false
        }
        fn left(&self) {
            self.0.left.set(self.0.left.get() + 1);
        }
        fn dropped(&self, transfer: Option<ops::Transfer>) {
            *self.0.transfer.borrow_mut() = transfer;
        }
    }

    thread_local! {static TIMEOUT:Cell<bool>=const{Cell::new(false)};}
    unsafe extern "system" fn cancel_probe_menu(_: HWND, _: u32, id: usize, _: u32) {
        TIMEOUT.with(|value| value.set(true));
        if let Err(error) = windows::Win32::UI::WindowsAndMessaging::KillTimer(None, id) {
            eprintln!("drop probe timer: {error}");
        }
        if let Err(error) = windows::Win32::UI::WindowsAndMessaging::EndMenu() {
            eprintln!("drop probe menu cancel: {error}");
        }
    }

    #[test]
    fn same_folder_background_move_is_forbidden_but_copy_link_and_children_remain_targets() {
        let mut target = Target {
            folder: r"C:\Source".into(),
            item: None,
            spot: None,
        };
        assert!(same_folder_move(
            &target,
            Some(r"C:\Source"),
            MODIFIERKEYS_FLAGS(1)
        ));
        assert!(same_folder_move(
            &target,
            Some(r"C:\Source"),
            MODIFIERKEYS_FLAGS(5)
        ));
        for keys in [9, 13, 33] {
            assert!(!same_folder_move(
                &target,
                Some(r"C:\Source"),
                MODIFIERKEYS_FLAGS(keys)
            ));
        }
        assert!(!same_folder_move(
            &target,
            Some(r"C:\Other"),
            MODIFIERKEYS_FLAGS(1)
        ));
        assert!(!same_folder_move(&target, None, MODIFIERKEYS_FLAGS(1)));
        target.item = Some(r"C:\Source\Child".into());
        assert!(!same_folder_move(
            &target,
            Some(r"C:\Source"),
            MODIFIERKEYS_FLAGS(1)
        ));
    }

    #[test]
    fn shell_drop_copy_move_retarget_cancel_and_shared_history_preserve_owned_bytes() {
        unsafe {
            OleInitialize(None).unwrap();
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("sw-native-drop17-{}-{unique}", std::process::id()));
        let source = root.join("Source");
        let first = root.join("First");
        let destination = root.join("Destination");
        for directory in [&source, &first, &destination] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let original = source.join("Owned drop17.txt");
        let contents = b"Owned task17 native OLE drop bytes\0\xff\x10";
        std::fs::write(&original, contents).unwrap();
        let paths = vec![original.to_string_lossy().into_owned()];
        let state = Rc::new(Probe {
            destination: RefCell::new(Target {
                folder: first.to_string_lossy().into_owned(),
                item: None,
                spot: Some(Spot::Row(0)),
            }),
            transfer: RefCell::new(None),
            entered: Cell::new(0),
            left: Cell::new(0),
        });
        let target: IDropTarget = WindowDropTarget {
            hwnd: HWND::default(),
            resolver: Rc::new(Adapter(state.clone())),
            helper: None,
            data: RefCell::new(None),
            source_parent: RefCell::new(None),
            same_folder: std::cell::Cell::new(false),
            current: RefCell::new(None),
            rejected: RefCell::new(None),
            caption: RefCell::new(String::new()),
            image_hidden: Cell::new(false),
        }
        .into();
        let at = POINTL { x: 0, y: 0 };
        for (copy, commit) in [(true, true), (false, true), (false, false)] {
            *state.destination.borrow_mut() = Target {
                folder: first.to_string_lossy().into_owned(),
                item: None,
                spot: Some(Spot::Row(0)),
            };
            let data = ops::data_object(HWND::default(), &paths).unwrap();
            let keys = MODIFIERKEYS_FLAGS(if copy { 9 } else { 1 });
            let mut effect = DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK;
            unsafe {
                target.DragEnter(&data, keys, at, &mut effect).unwrap();
            }
            assert_eq!(state.entered.get(), 1);
            *state.destination.borrow_mut() = Target {
                folder: destination.to_string_lossy().into_owned(),
                item: None,
                spot: Some(Spot::Crumb(0)),
            };
            effect = DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK;
            unsafe {
                target.DragOver(keys, at, &mut effect).unwrap();
            }
            assert_eq!(
                effect,
                if copy {
                    DROPEFFECT_COPY
                } else {
                    DROPEFFECT_MOVE
                }
            );
            if commit {
                TIMEOUT.with(|value| value.set(false));
                let timer = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::SetTimer(
                        None,
                        0,
                        10000,
                        Some(cancel_probe_menu),
                    )
                };
                assert_ne!(timer, 0);
                unsafe {
                    target
                        .Drop(&data, MODIFIERKEYS_FLAGS(keys.0 & !1), at, &mut effect)
                        .unwrap();
                }
                let timed_out = TIMEOUT.with(Cell::get);
                if !timed_out {
                    unsafe {
                        windows::Win32::UI::WindowsAndMessaging::KillTimer(None, timer).unwrap();
                    }
                }
                assert!(!timed_out, "Shell drop opened an unexpected modal menu");
                let made = destination.join("Owned drop17.txt");
                assert_eq!(std::fs::read(&made).unwrap(), contents);
                assert_eq!(original.exists(), copy);
                assert!(std::fs::read_dir(&first).unwrap().next().is_none());
                let transfer = state.transfer.borrow_mut().take().unwrap();
                assert_eq!(transfer.copied.len(), usize::from(copy));
                assert_eq!(transfer.moved.len(), usize::from(!copy));
                let operation = crate::app::undo::transfers(transfer).pop().unwrap();
                let applied = crate::app::undo::undo_operation(HWND::default(), operation).unwrap();
                assert_eq!(std::fs::read(&original).unwrap(), contents);
                assert!(!made.exists());
                let applied =
                    crate::app::undo::redo_operation(HWND::default(), applied.operation).unwrap();
                assert_eq!(std::fs::read(&made).unwrap(), contents);
                crate::app::undo::undo_operation(HWND::default(), applied.operation).unwrap();
                assert_eq!(std::fs::read(&original).unwrap(), contents);
                assert!(!made.exists());
            } else {
                unsafe {
                    target.DragLeave().unwrap();
                }
                assert_eq!(state.left.get(), 1);
                assert!(std::fs::read_dir(&destination).unwrap().next().is_none());
                assert_eq!(std::fs::read(&original).unwrap(), contents);
            }
        }
        drop(target);
        std::fs::remove_file(original).unwrap();
        for directory in [source, first, destination, PathBuf::from(root)] {
            std::fs::remove_dir(directory).unwrap();
        }
        unsafe {
            OleUninitialize();
        }
    }
}
