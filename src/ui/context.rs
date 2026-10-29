use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use windows::core::Result;
use windows::Win32::Foundation::HWND;

use super::render::{Damage, Renderer};

pub(super) struct Context {
    window: RefCell<Option<super::Window>>,
    damage: Cell<Damage>,
    list_pending: Cell<bool>,
    nav_pending: Cell<bool>,
    address_pending: Cell<bool>,
    preferences_pending: Cell<bool>,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            window: RefCell::new(None),
            damage: Cell::new(Damage::ALL),
            list_pending: Cell::new(false),
            nav_pending: Cell::new(false),
            address_pending: Cell::new(false),
            preferences_pending: Cell::new(false),
        }
    }
}

thread_local! {
    static WINDOWS: RefCell<HashMap<usize, Rc<Context>>> = RefCell::new(HashMap::new());
    static CURRENT: Cell<usize> = const { Cell::new(0) };
    static EMPTY: Rc<Context> = Rc::new(Context::default());
    static RENDERER: RefCell<Weak<Renderer>> = const { RefCell::new(Weak::new()) };
}

pub(super) struct Local<T>(fn(&Context) -> &T);

impl<T> Local<T> {
    pub fn with<R>(&self, action: impl FnOnce(&T) -> R) -> R {
        let key = CURRENT.with(Cell::get);
        let context = WINDOWS
            .with(|windows| windows.borrow().get(&key).cloned())
            .unwrap_or_else(|| EMPTY.with(Rc::clone));
        action((self.0)(&context))
    }
}

pub(super) const WINDOW: Local<RefCell<Option<super::Window>>> = Local(|context| &context.window);
pub(super) const DAMAGE: Local<Cell<Damage>> = Local(|context| &context.damage);
pub(super) const LIST_PENDING: Local<Cell<bool>> = Local(|context| &context.list_pending);
pub(super) const NAV_PENDING: Local<Cell<bool>> = Local(|context| &context.nav_pending);
pub(super) const ADDRESS_PENDING: Local<Cell<bool>> = Local(|context| &context.address_pending);
pub(super) const PREFERENCES_PENDING: Local<Cell<bool>> =
    Local(|context| &context.preferences_pending);

pub(super) struct Enter(usize);

pub(super) fn enter(hwnd: HWND) -> Enter {
    let key = hwnd.0 as usize;
    Enter(CURRENT.with(|current| current.replace(key)))
}

pub(super) fn register(hwnd: HWND) {
    WINDOWS.with(|windows| {
        windows
            .borrow_mut()
            .insert(hwnd.0 as usize, Rc::new(Context::default()));
    });
}

impl Drop for Enter {
    fn drop(&mut self) {
        CURRENT.with(|current| current.set(self.0));
    }
}

pub(super) fn remove(hwnd: HWND) {
    WINDOWS.with(|windows| windows.borrow_mut().remove(&(hwnd.0 as usize)));
}

pub(super) fn handles() -> Vec<HWND> {
    WINDOWS.with(|windows| windows.borrow().keys().map(|key| HWND(*key as _)).collect())
}

pub(super) fn hwnd() -> HWND {
    HWND(CURRENT.with(Cell::get) as _)
}

pub(super) fn renderer() -> Result<Rc<Renderer>> {
    if let Some(renderer) = RENDERER.with(|renderer| renderer.borrow().upgrade()) {
        return Ok(renderer);
    }
    let renderer = Rc::new(Renderer::new()?);
    RENDERER.with(|shared| *shared.borrow_mut() = Rc::downgrade(&renderer));
    Ok(renderer)
}

pub(super) fn reset_renderer() {
    RENDERER.with(|renderer| *renderer.borrow_mut() = Weak::new());
    let contexts: Vec<_> = WINDOWS.with(|windows| {
        windows
            .borrow()
            .iter()
            .map(|(key, context)| (*key, context.clone()))
            .collect()
    });
    for (key, context) in contexts {
        context.damage.set(Damage::ALL);
        if !unsafe {
            windows::Win32::Graphics::Gdi::InvalidateRect(Some(HWND(key as _)), None, false)
        }
        .as_bool()
        {
            eprintln!(
                "native device recovery invalidation: {}",
                windows::core::Error::from_win32()
            );
        }
    }
}
