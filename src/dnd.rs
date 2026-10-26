//! Shell drag and drop. Drags start from the selection with SHDoDragDrop; drops on the window
//! are forwarded to the shell's own drop target for the folder under the pointer (a folder row,
//! or the folder being shown), so copy/move/link and their prompts behave like Explorer's.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::core::{implement, w, Ref, Result};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, POINTL, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    CoCreateInstance, IDataObject, CLSCTX_INPROC_SERVER, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, OleInitialize, OleUninitialize, RegisterDragDrop, ReleaseStgMedium, DROPEFFECT,
    DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::Shell::{
    CLSID_DragDropHelper, IDropTargetHelper, SHDoDragDrop, DROPDESCRIPTION, DROPIMAGE_INVALID, DROPIMAGE_NOIMAGE,
};
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, ShowWindow, ShowWindowAsync, SW_HIDE, SW_SHOWNA, WM_KEYDOWN};

use crate::shell::ops;

/// What a drop lands on, to light it up: a row of the view, a row of the navigation pane
/// or a crumb of the address bar.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Spot {
    Row(usize),
    Nav(usize),
    Crumb(usize),
    Pin {at:usize,line:i32,row:Option<usize>},
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

thread_local! {
    static RESOLVER: RefCell<Option<Box<dyn Resolver>>> = const { RefCell::new(None) };
}

fn with_resolver<T>(f: impl FnOnce(&dyn Resolver) -> T) -> Option<T> {
    RESOLVER.with(|r| r.borrow().as_ref().map(|r| f(r.as_ref())))
}

struct Current {
    target: Target,
    drop: IDropTarget,
}

#[implement(IDropTarget)]
struct WindowDropTarget {
    hwnd: HWND,
    helper: Option<IDropTargetHelper>,
    data: RefCell<Option<IDataObject>>,
    current: RefCell<Option<Current>>,
    /// The caption the crumb's shell target last asked for.
    caption: RefCell<String>,
    /// The system drag image is hidden (the bar draws its own over the crumbs).
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

    /// Re-targets to the folder under the pointer and forwards the event to its drop target.
    fn retarget(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) {
        let target = with_resolver(|r| r.target_at(self.client(pt))).flatten();
        let mut current = self.current.borrow_mut();
        let same = match (&*current, &target) {
            (Some(c), Some(t)) => c.target == *t,
            (None, None) => true,
            _ => false,
        };
        unsafe {
            if same {
                match &*current {
                    Some(c) => {
                        if c.drop.DragOver(keys, *pt, effect).is_err() {
                            *effect = DROPEFFECT_NONE;
                        }
                    }
                    None => *effect = DROPEFFECT_NONE,
                }
                return;
            }
            if let Some(old) = current.take() {
                let _ = old.drop.DragLeave();
            }
            let spot = target.as_ref().and_then(|t| t.spot);
            let _ = with_resolver(|r| r.highlight(spot));
            let Some(target) = target else {
                *effect = DROPEFFECT_NONE;
                return;
            };
            let data = self.data.borrow().clone();
            let drop=match target.spot {
                Some(Spot::Pin{at,..})=>Ok(crate::shell::quick_access::drop_target(at)),
                _=>ops::drop_target(self.hwnd, &target.folder, target.item.as_deref()),
            };
            match drop {
                Ok(drop) => {
                    if drop.DragEnter(data.as_ref(), keys, *pt, effect).is_err() {
                        *effect = DROPEFFECT_NONE;
                    }
                    *current = Some(Current { target, drop });
                }
                Err(_) => *effect = DROPEFFECT_NONE,
            }
        }
    }
}

impl WindowDropTarget {
    /// Over a crumb the shell target's caption moves from the drag image to the bar.
    fn update_caption(&self, pt: &POINTL, effect: DROPEFFECT) {
        let crumb = matches!(self.current.borrow().as_ref().and_then(|c| c.target.spot), Some(Spot::Crumb(_)));
        let at = self.client(pt);
        let tab = with_resolver(|r| r.tab_preview(at)).unwrap_or(false);
        let data = self.data.borrow().clone();
        let caption = if tab {
            Some((String::new(), DROPEFFECT_NONE, at))
        } else {
            match data {
                Some(data) if crumb => {
                    if let Some(text) = take_description(&data) {
                        *self.caption.borrow_mut() = text;
                    }
                    let text = self.caption.borrow().clone();
                    (effect != DROPEFFECT_NONE && !text.is_empty()).then(|| (text, effect, self.client(pt)))
                }
                _ => None,
            }
        };
        let hide = caption.is_some();
        if !hide {
            self.show_image();
        }
        self.image_hidden.set(hide);
        let _ = with_resolver(|r| r.caption(caption));
    }
}

/// The drag image's window, which the drag image manager keeps in the data object.
fn drag_window(data: &IDataObject) -> Option<HWND> {
    unsafe {
        let format = RegisterClipboardFormatW(w!("DragWindow")) as u16;
        let fmt = FORMATETC { cfFormat: format, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
        let mut medium = data.GetData(&fmt).ok()?;
        let global = medium.u.hGlobal;
        let ptr = GlobalLock(global) as *const u32;
        let window = (!ptr.is_null()).then(|| HWND(std::ptr::read_unaligned(ptr) as usize as *mut _));
        let _ = GlobalUnlock(global);
        ReleaseStgMedium(&mut medium);
        window.filter(|w| !w.is_invalid())
    }
}


impl WindowDropTarget {
    /// Explorer's XAML bar draws the dragged item itself, under its caption: over a crumb the
    /// drag image's own window is hidden, after each move the image helper made.
    fn hide_image(&self) {
        if self.image_hidden.get() {
            if let Some(window) = self.data.borrow().as_ref().and_then(drag_window) {
                unsafe {
                    let _ = ShowWindow(window, SW_HIDE);
                }
            }
        }
    }

    /// The drag leaves a crumb (or the window, or drops): the drag image's window comes back.
    fn show_image(&self) {
        if self.image_hidden.replace(false) {
            if let Some(window) = self.data.borrow().as_ref().and_then(drag_window) {
                unsafe {
                    let _ = ShowWindowAsync(window, SW_SHOWNA);
                }
            }
        }
    }
}

/// The drop description a target set ("Mover para %1" with its insert), as text, leaving
/// the drag image without one. None when there is no new one.
fn take_description(data: &IDataObject) -> Option<String> {
    unsafe {
        let format = RegisterClipboardFormatW(w!("DropDescription")) as u16;
        let fmt = FORMATETC { cfFormat: format, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
        let mut medium = data.GetData(&fmt).ok()?;
        let global = medium.u.hGlobal;
        let ptr = GlobalLock(global) as *const DROPDESCRIPTION;
        let description = (!ptr.is_null()).then(|| std::ptr::read_unaligned(ptr));
        let _ = GlobalUnlock(global);
        ReleaseStgMedium(&mut medium);
        let description = description?;
        let (kind, message, insert) = (description.r#type, description.szMessage, description.szInsert);
        if kind == DROPIMAGE_NOIMAGE || kind == DROPIMAGE_INVALID {
            return None;
        }
        let text = |w: &[u16]| String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())]);
        let caption = text(&message).replace("%1", &text(&insert));

        let global = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, std::mem::size_of::<DROPDESCRIPTION>()).ok()?;
        let blank = GlobalLock(global) as *mut DROPDESCRIPTION;
        if !blank.is_null() {
            std::ptr::addr_of_mut!((*blank).r#type).write_unaligned(DROPIMAGE_NOIMAGE);
            let _ = GlobalUnlock(global);
            let medium = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: global }, pUnkForRelease: std::mem::ManuallyDrop::new(None) };
            let _ = data.SetData(&fmt, &medium, true);
        }
        (!caption.is_empty()).then_some(caption)
    }
}

impl IDropTarget_Impl for WindowDropTarget_Impl {
    fn DragEnter(&self, data: Ref<'_, IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        *self.data.borrow_mut() = data.cloned();
        self.image_hidden.set(false);
        let files = data.as_ref().map(ops::drag_items).unwrap_or_default();
        // Explorer's XAML source leaves a blank legacy helper bitmap over another
        // application's window. Supply the same shell folder picture for that
        // standard OLE helper, as we already do for drags started in this app.
        if DRAG_THREAD.load(Ordering::SeqCst)==0 && files.len()==1 && std::path::Path::new(&files[0]).is_dir() {
            if let Some(data)=data.as_ref(){let _=initialize_drag_picture(data,&files[0]);}
        }
        let _ = with_resolver(|r| r.entered(files));
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

    fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
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
        if let Some(old) = self.current.borrow_mut().take() {
            unsafe {
                let _ = old.drop.DragLeave();
            }
        }
        self.show_image();
        *self.data.borrow_mut() = None;
        let _ = with_resolver(|r| {
            r.highlight(None);
            r.caption(None);
            r.left();
        });
        if let Some(helper) = &self.helper {
            unsafe {
                let _ = helper.DragLeave();
            }
        }
        Ok(())
    }

    fn Drop(&self, data: Ref<'_, IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
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
            // Files into a folder: done at once, and what they became told apart, for
            // "Desfazer Mover" and "Desfazer Copiar".
            let dest = c.target.item.clone().unwrap_or_else(|| c.target.folder.clone());
            let sources = data.as_ref().map(ops::dropped_files).unwrap_or_default();
            let before = (!sources.is_empty() && std::path::Path::new(&dest).is_dir()).then(|| ops::snapshot(&dest)).flatten();
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
        *self.data.borrow_mut() = None;
        let _ = with_resolver(|r| {
            r.highlight(None);
            r.caption(None);
            r.dropped(transfer);
        });
        Ok(())
    }
}

/// OLE must be initialized on the UI thread before the window registers as a drop target.
pub fn init_ole() {
    unsafe {
        let _ = OleInitialize(None);
    }
}

pub fn register(hwnd: HWND, resolver: Box<dyn Resolver>) {
    RESOLVER.with(|r| *r.borrow_mut() = Some(resolver));
    let helper: Option<IDropTargetHelper> = unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok() };
    let target: IDropTarget = WindowDropTarget {
        hwnd,
        helper,
        data: RefCell::new(None),
        current: RefCell::new(None),
        caption: RefCell::new(String::new()),
        image_hidden: std::cell::Cell::new(false),
    }
    .into();
    unsafe {
        let _ = RegisterDragDrop(hwnd, &target);
    }
}

/// Drags the given items on a thread of their own, sharing the UI thread's input state, so
/// the window keeps painting (drop highlights, opening tree rows) while the OLE drag loop
/// runs; `done` gets the effect there.
pub fn drag_apart(hwnd: HWND, paths: Vec<String>, done: impl FnOnce(DROPEFFECT) + Send + 'static) {
    let ui_thread = unsafe { GetCurrentThreadId() };
    let window = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        let _ = OleInitialize(None);
        let me = GetCurrentThreadId();
        DRAG_THREAD.store(me, Ordering::SeqCst);
        let _ = AttachThreadInput(me, ui_thread, true);
        let effect = drag(HWND(window as *mut _), &paths);
        let _ = AttachThreadInput(me, ui_thread, false);
        DRAG_THREAD.store(0, Ordering::SeqCst);
        OleUninitialize();
        done(effect);
    });
}

/// The thread running a drag of ours (0: none).
static DRAG_THREAD: AtomicU32 = AtomicU32::new(0);

/// Escape reached the window, whose thread has the keyboard: it goes on to the drag's loop,
/// which ends the drag (true when there was one).
pub fn cancel_drag() -> bool {
    let thread = DRAG_THREAD.load(Ordering::SeqCst);
    thread != 0 && unsafe { PostThreadMessageW(thread, WM_KEYDOWN, WPARAM(VK_ESCAPE.0 as usize), LPARAM(0)).is_ok() }
}

/// Drags the given items (all in `folder`) with the shell's data object and drag image.
fn drag(hwnd: HWND, paths: &[String]) -> DROPEFFECT {
    let Ok(data) = ops::data_object(hwnd, paths) else { return DROPEFFECT_NONE };
    if let Some(path) = paths.first().filter(|_| paths.len() == 1) {
        let _ = initialize_drag_picture(&data, path);
    }
    unsafe { SHDoDragDrop(Some(hwnd), &data, None, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK).unwrap_or(DROPEFFECT_NONE) }
}

fn initialize_drag_picture(data: &IDataObject, path: &str) -> Result<()> {
    use windows::Win32::Foundation::{COLORREF, SIZE};
    use windows::Win32::Graphics::Gdi::{CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS};
    use windows::Win32::UI::Shell::{IDragSourceHelper2, SHDRAGIMAGE, DSH_ALLOWDROPDESCRIPTIONTEXT};
    let picture = crate::shell::icons::drag_picture(path, 96).map_err(|_| windows::core::Error::from_win32())?;
    unsafe {
        set_drag_flag(data, "InShellDragLoop", true)?;
        let helper: IDragSourceHelper2 = CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER)?;
        helper.SetFlags(DSH_ALLOWDROPDESCRIPTIONTEXT.0 as u32)?;
        let info = BITMAPINFO { bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: picture.width as i32, biHeight: -(picture.height as i32),
            biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default()
        }, ..Default::default() };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let output = std::slice::from_raw_parts_mut(bits as *mut u8, picture.pixels.len());
        for (src, dst) in picture.pixels.chunks_exact(4).zip(output.chunks_exact_mut(4)) {
            dst.copy_from_slice(&[src[2], src[1], src[0], src[3]]);
        }
        let image = SHDRAGIMAGE { sizeDragImage: SIZE { cx: picture.width as i32, cy: picture.height as i32 },
            ptOffset: POINT { x: picture.width as i32 / 2, y: picture.height as i32 - 7 },
            hbmpDragImage: bitmap, crColorKey: COLORREF(0xFFFF_FFFF) };
        let result = helper.InitializeFromBitmap(&image, data);
        // On success the shell image manager owns the bitmap.
        if result.is_err() { let _ = DeleteObject(bitmap.into()); }
        result?;
        set_drag_flag(data, "IsShowingText", true)?;
        set_drag_flag(data, "UsingDefaultDragImage", true)
    }
}

unsafe fn set_drag_flag(data: &IDataObject, name: &str, value: bool) -> Result<()> {
    let name = crate::shell::wide(name);
    let format = RegisterClipboardFormatW(windows::core::PCWSTR(name.as_ptr())) as u16;
    let fmt = FORMATETC { cfFormat: format, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    let global = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, 4)?;
    let ptr = GlobalLock(global) as *mut u32;
    if ptr.is_null() {
        let error = windows::core::Error::from_win32();
        let _ = windows::Win32::Foundation::GlobalFree(Some(global));
        return Err(error);
    }
    ptr.write(u32::from(value));
    let _ = GlobalUnlock(global);
    let mut medium = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: global }, pUnkForRelease: std::mem::ManuallyDrop::new(None) };
    let result = data.SetData(&fmt, &medium, true);
    if result.is_err() { ReleaseStgMedium(&mut medium); }
    result
}

