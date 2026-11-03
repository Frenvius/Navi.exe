use notify::Watcher;
use std::{
    cell::OnceCell,
    ffi::c_void,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use windows::{
    core::{IUnknown, IUnknown_Vtbl, Interface, GUID, HRESULT},
    Win32::{
        Foundation::*,
        System::{Com::*, Threading::*},
        UI::{Shell::Common::ITEMIDLIST, WindowsAndMessaging::*},
    },
};
const CLSID: GUID = GUID::from_u128(0x75dff2b7_6936_4c06_a8bb_676a7b00b24b);
const HOST: GUID = GUID::from_u128(0xff9ce68e_060d_4120_81f9_f3061c703225);
const HOST2: GUID = GUID::from_u128(0x7080b61c_7b9a_4910_9281_8f4b980ce39f);
const NATIVE: GUID = GUID::from_u128(0xd7dfb84a_c031_49bc_ad7c_5064810a2e74);
static SERVER_LOCKS: AtomicU32 = AtomicU32::new(0);
static SERVER_REFS: AtomicU32 = AtomicU32::new(0);
static SERVER_THREAD: AtomicU32 = AtomicU32::new(0);
static STOPPING: AtomicBool = AtomicBool::new(false);
const STOP_MESSAGE: u32 = WM_APP + 0x381;
const RELEASE_MESSAGE: u32 = WM_APP + 0x382;
#[cfg(test)]
static TEST_SERVER: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct ServerPin;
impl ServerPin {
    fn new() -> Self {
        unsafe { CoAddRefServerProcess() };
        SERVER_REFS.fetch_add(1, Ordering::AcqRel);
        Self
    }
}
impl Drop for ServerPin {
    fn drop(&mut self) {
        let remaining = SERVER_REFS.fetch_sub(1, Ordering::AcqRel) - 1;
        unsafe {
            CoReleaseServerProcess();
            if remaining == 0 {
                let _ = PostThreadMessageW(
                    SERVER_THREAD.load(Ordering::Acquire),
                    RELEASE_MESSAGE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }
}
#[repr(C)]
struct HostVtbl {
    base: IUnknown_Vtbl,
    show: unsafe extern "system" fn(
        *mut c_void,
        *const ITEMIDLIST,
        i32,
        POINT,
        i32,
        HWND,
        *mut c_void,
        *mut c_void,
    ) -> HRESULT,
    rect:
        unsafe extern "system" fn(*mut c_void, *const ITEMIDLIST, i32, *const RECT, i32) -> HRESULT,
}

#[repr(C)]
struct TabsVtbl {
    base: IUnknown_Vtbl,
    show: unsafe extern "system" fn(
        *mut c_void,
        *const ITEMIDLIST,
        *mut c_void,
        *const RECT,
        i32,
        i32,
    ) -> HRESULT,
}
#[repr(C)]
struct TabsObject {
    vtable: *const TabsVtbl,
    owner: *mut HostObject,
}

#[repr(C)]
struct HostObject {
    vtable: *const HostVtbl,
    refs: AtomicU32,
    native: OnceCell<IUnknown>,
    factory: Option<IClassFactory>,
    tabs: TabsObject,
    _server: ServerPin,
}
impl HostObject {
    unsafe fn native(&self) -> windows::core::Result<IUnknown> {
        if let Some(native) = self.native.get() {
            return Ok(native.clone());
        }
        let factory = self
            .factory
            .as_ref()
            .ok_or_else(|| windows::core::Error::from_hresult(E_UNEXPECTED))?;
        let object = factory.CreateInstance::<_, IUnknown>(None)?;
        let _ = self.native.set(object);
        Ok(self.native.get().unwrap().clone())
    }
}

unsafe extern "system" fn host_query(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    *out = std::ptr::null_mut();

    if [IUnknown::IID, HOST, HOST2].contains(&*iid) {
        *out = if *iid == HOST2 {
            (&mut (*(this as *mut HostObject)).tabs as *mut TabsObject).cast()
        } else {
            this
        };
        host_addref(this);
        return S_OK;
    }
    if (*iid).data2 == 0 && (*iid).data3 == 0 && (*iid).data4 == [0xc0, 0, 0, 0, 0, 0, 0, 0x46] {
        return E_NOINTERFACE;
    }
    E_NOINTERFACE
}
unsafe extern "system" fn host_addref(this: *mut c_void) -> u32 {
    (*(this as *mut HostObject))
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}
unsafe extern "system" fn host_release(this: *mut c_void) -> u32 {
    let left = (*(this as *mut HostObject))
        .refs
        .fetch_sub(1, Ordering::AcqRel)
        - 1;
    if left == 0 {
        drop(Box::from_raw(this as *mut HostObject));
    }
    left
}
unsafe extern "system" fn host_show(
    this: *mut c_void,
    pidl: *const ITEMIDLIST,
    flags: i32,
    point: POINT,
    show: i32,
    owner: HWND,
    site: *mut c_void,
    handshake: *mut c_void,
) -> HRESULT {
    super::trace(
        "show",
        serde_json::json!({"flags":flags,"show":show,"point":[point.x,point.y],"owner":owner.0 as usize,"site":!site.is_null(),"handshake":!handshake.is_null()}),
    );
    if !pidl.is_null()
        && !STOPPING.load(Ordering::Acquire)
        && matches!(flags, 0 | 4 | 8)
        && show == 10
        && point.x == 0
        && point.y == 0
        && owner.is_invalid()
        && site.is_null()
        && handshake.is_null()
        && caller_allowed()
        && super::admit(pidl, flags)
    {
        return S_OK;
    }
    let native = match (*(this as *mut HostObject)).native() {
        Ok(native) => native,
        Err(error) => return error.code(),
    };
    let mut raw = std::ptr::null_mut();
    let result = (native.vtable().QueryInterface)(native.as_raw(), &HOST, &mut raw);
    if result.is_err() {
        return result;
    }
    let object = IUnknown::from_raw(raw);
    let table = &**(raw as *const *const HostVtbl);
    let result = (table.show)(
        object.as_raw(),
        pidl,
        flags,
        point,
        show,
        owner,
        site,
        handshake,
    );
    super::trace("native-forward", serde_json::json!({"hr":result.0}));
    result
}
unsafe fn caller_allowed() -> bool {
    if CoImpersonateClient().is_err() {
        return false;
    }
    let mut token = HANDLE::default();
    let result = if OpenThreadToken(
        GetCurrentThread(),
        windows::Win32::Security::TOKEN_QUERY,
        true,
        &mut token,
    )
    .is_ok()
    {
        let token = crate::explorer_host::transport::Owned(token);
        crate::explorer_host::transport::token_identity(token.0).ok()
    } else {
        None
    };
    let reverted = CoRevertToSelf().is_ok();
    reverted
        && result.is_some()
        && result == crate::explorer_host::transport::identity(std::process::id()).ok()
}
unsafe extern "system" fn host_rect(
    this: *mut c_void,
    pidl: *const ITEMIDLIST,
    flags: i32,
    rect: *const RECT,
    show: i32,
) -> HRESULT {
    super::trace(
        "rect",
        serde_json::json!({"flags":flags,"show":show,"rectangle":rect.as_ref().map(|rect|[rect.left,rect.top,rect.right,rect.bottom])}),
    );
    let native = match (*(this as *mut HostObject)).native() {
        Ok(native) => native,
        Err(error) => return error.code(),
    };
    let mut raw = std::ptr::null_mut();
    let result = (native.vtable().QueryInterface)(native.as_raw(), &HOST, &mut raw);
    if result.is_err() {
        return result;
    }
    let object = IUnknown::from_raw(raw);
    let table = &**(raw as *const *const HostVtbl);
    let result = (table.rect)(object.as_raw(), pidl, flags, rect, show);

    result
}
static HOST_TABLE: HostVtbl = HostVtbl {
    base: IUnknown_Vtbl {
        QueryInterface: host_query,
        AddRef: host_addref,
        Release: host_release,
    },
    show: host_show,
    rect: host_rect,
};

unsafe extern "system" fn tabs_query(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    host_query((*(this as *mut TabsObject)).owner.cast(), iid, out)
}
unsafe extern "system" fn tabs_addref(this: *mut c_void) -> u32 {
    host_addref((*(this as *mut TabsObject)).owner.cast())
}
unsafe extern "system" fn tabs_release(this: *mut c_void) -> u32 {
    host_release((*(this as *mut TabsObject)).owner.cast())
}
unsafe extern "system" fn tabs_show(
    this: *mut c_void,
    pidl: *const ITEMIDLIST,
    items: *mut c_void,
    rect: *const RECT,
    show: i32,
    flags: i32,
) -> HRESULT {
    super::trace(
        "tabs",
        serde_json::json!({"flags":flags,"show":show,"rectangle":rect.as_ref().map(|rect|[rect.left,rect.top,rect.right,rect.bottom])}),
    );
    let owner = &*(*(this as *mut TabsObject)).owner;
    let native = match owner.native() {
        Ok(native) => native,
        Err(error) => return error.code(),
    };
    let mut raw = std::ptr::null_mut();
    let result = (native.vtable().QueryInterface)(native.as_raw(), &HOST2, &mut raw);
    if result.is_err() {
        return result;
    }
    let object = IUnknown::from_raw(raw);
    let table = &**(raw as *const *const TabsVtbl);
    let result = (table.show)(object.as_raw(), pidl, items, rect, show, flags);

    result
}
static TABS_TABLE: TabsVtbl = TabsVtbl {
    base: IUnknown_Vtbl {
        QueryInterface: tabs_query,
        AddRef: tabs_addref,
        Release: tabs_release,
    },
    show: tabs_show,
};

#[repr(C)]
struct FactoryObject {
    vtable: *const IClassFactory_Vtbl,
    refs: AtomicU32,
    native: Option<IClassFactory>,
}
unsafe extern "system" fn factory_query(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    *out = std::ptr::null_mut();

    if [IUnknown::IID, IClassFactory::IID].contains(&*iid) {
        *out = this;
        factory_addref(this);
        S_OK
    } else {
        E_NOINTERFACE
    }
}
unsafe extern "system" fn factory_addref(this: *mut c_void) -> u32 {
    (*(this as *mut FactoryObject))
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}
unsafe extern "system" fn factory_release(this: *mut c_void) -> u32 {
    let left = (*(this as *mut FactoryObject))
        .refs
        .fetch_sub(1, Ordering::AcqRel)
        - 1;
    if left == 0 {
        drop(Box::from_raw(this as *mut FactoryObject));
    }
    left
}
unsafe extern "system" fn factory_create(
    this: *mut c_void,
    outer: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    *out = std::ptr::null_mut();

    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    if STOPPING.load(Ordering::Acquire)
        || !crate::explorer_host::version::verified()
        || ![IUnknown::IID, HOST, HOST2].contains(&*iid)
    {
        let Some(native) = &(*(this as *mut FactoryObject)).native else {
            return E_UNEXPECTED;
        };
        return (native.vtable().CreateInstance)(native.as_raw(), outer, iid, out);
    }
    let object = Box::into_raw(Box::new(HostObject {
        vtable: &HOST_TABLE,
        refs: AtomicU32::new(1),
        native: OnceCell::new(),
        factory: (*(this as *mut FactoryObject)).native.clone(),
        tabs: TabsObject {
            vtable: &TABS_TABLE,
            owner: std::ptr::null_mut(),
        },
        _server: ServerPin::new(),
    }));
    (*object).tabs.owner = object;
    let object = object.cast();
    let result = host_query(object, iid, out);
    host_release(object);

    result
}
unsafe extern "system" fn factory_lock(this: *mut c_void, lock: windows::core::BOOL) -> HRESULT {
    let Some(native) = &(*(this as *mut FactoryObject)).native else {
        return E_UNEXPECTED;
    };
    let pin = lock.as_bool().then(ServerPin::new);
    if let Err(error) = native.LockServer(lock.as_bool()) {
        return error.code();
    }
    if lock.as_bool() {
        SERVER_LOCKS.fetch_add(1, Ordering::AcqRel);
        std::mem::forget(pin);
    } else if SERVER_LOCKS
        .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            count.checked_sub(1)
        })
        .is_ok()
    {
        drop(ServerPin);
    }
    S_OK
}
static FACTORY_TABLE: IClassFactory_Vtbl = IClassFactory_Vtbl {
    base__: IUnknown_Vtbl {
        QueryInterface: factory_query,
        AddRef: factory_addref,
        Release: factory_release,
    },
    CreateInstance: factory_create,
    LockServer: factory_lock,
};

pub fn serve(seconds: u64, stop_name: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        SERVER_THREAD.store(GetCurrentThreadId(), Ordering::Release);
        STOPPING.store(false, Ordering::Release);
        let mut registration = Some(ServerPin::new());
        let native: IClassFactory = CoGetClassObject(&NATIVE, CLSCTX_LOCAL_SERVER, None)?;
        let factory = IUnknown::from_raw(
            Box::into_raw(Box::new(FactoryObject {
                vtable: &FACTORY_TABLE,
                refs: AtomicU32::new(1),
                native: Some(native),
            }))
            .cast(),
        );
        let cookie =
            CoRegisterClassObject(&CLSID, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE)?;
        let thread = GetCurrentThreadId();
        let mut message = MSG::default();
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        let mut draining =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok() && crate::explorer_host::install::draining() {
                    let _ = PostThreadMessageW(thread, STOP_MESSAGE, WPARAM(0), LPARAM(0));
                }
            })?;
        draining.watch(
            &crate::explorer_host::install::directory()?,
            notify::RecursiveMode::NonRecursive,
        )?;
        if crate::explorer_host::install::draining() {
            let _ = PostThreadMessageW(thread, STOP_MESSAGE, WPARAM(0), LPARAM(0));
        }
        let identity = crate::explorer_host::transport::identity(std::process::id())?;
        let default_stop =
            crate::explorer_host::install::stop_event(&identity.sid, identity.session);
        let name = Some(
            stop_name
                .unwrap_or(&default_stop)
                .encode_utf16()
                .chain([0])
                .collect::<Vec<_>>(),
        );
        let stop = CreateEventW(
            None,
            true,
            false,
            name.as_ref()
                .map(|name| windows::core::PCWSTR(name.as_ptr()))
                .unwrap_or_default(),
        )?;
        let stop = stop.0 as usize;
        std::thread::spawn(move || {
            let stop = HANDLE(stop as _);
            let _ = WaitForSingleObject(
                stop,
                if seconds == 0 {
                    INFINITE
                } else {
                    (seconds * 1000) as u32
                },
            );
            let _ = CloseHandle(stop);
            let _ = PostThreadMessageW(thread, STOP_MESSAGE, WPARAM(0), LPARAM(0));
        });
        loop {
            let status = GetMessageW(&mut message, None, 0, 0).0;
            if status < 0 {
                return Err(windows::core::Error::from_win32().into());
            }
            if status == 0 || message.message == STOP_MESSAGE {
                if registration.is_some() {
                    STOPPING.store(true, Ordering::Release);
                    CoSuspendClassObjects()?;
                    CoRevokeClassObject(cookie)?;
                    registration.take();
                    super::trace(
                        "drain-begin",
                        serde_json::json!({"refs":SERVER_REFS.load(Ordering::Acquire),"locks":SERVER_LOCKS.load(Ordering::Acquire)}),
                    );
                }
            } else if message.message != RELEASE_MESSAGE {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            if registration.is_none() && SERVER_REFS.load(Ordering::Acquire) == 0 {
                break;
            }
        }
        super::trace("drain-end", serde_json::json!({}));
    }
    Ok(())
}
pub(super) struct NativeRequest(IUnknown);

#[cfg(test)]
mod lifetime_tests {
    use super::*;

    #[test]
    fn server_lifetime_remains_pinned_until_the_last_instance_releases() {
        let _lock = TEST_SERVER.lock().unwrap();
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap() };
        assert_eq!(SERVER_REFS.load(Ordering::Acquire), 0);
        let registration = ServerPin::new();
        let first = ServerPin::new();
        let second = ServerPin::new();
        assert_eq!(SERVER_REFS.load(Ordering::Acquire), 3);
        drop(registration);
        assert_eq!(SERVER_REFS.load(Ordering::Acquire), 2);
        drop(first);
        assert_eq!(SERVER_REFS.load(Ordering::Acquire), 1);
        drop(second);
        assert_eq!(SERVER_REFS.load(Ordering::Acquire), 0);
        unsafe { CoUninitialize() };
    }
}

impl NativeRequest {
    pub(super) fn prepare() -> windows::core::Result<Self> {
        unsafe {
            let object: IUnknown = CoCreateInstance(&NATIVE, None, CLSCTX_LOCAL_SERVER)?;
            let mut raw = std::ptr::null_mut();
            (object.vtable().QueryInterface)(object.as_raw(), &HOST, &mut raw).ok()?;
            Ok(Self(IUnknown::from_raw(raw)))
        }
    }
    pub(super) fn show(
        &self,
        request: &crate::explorer_host::protocol::Request,
        before_show: impl FnOnce() -> std::io::Result<()>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if !crate::explorer_host::protocol::valid_pidl(&request.pidl) {
            return Err("Invalid PIDL".into());
        }
        before_show()?;
        unsafe {
            let table = &**(self.0.as_raw() as *const *const HostVtbl);
            (table.show)(
                self.0.as_raw(),
                request.pidl.as_ptr().cast(),
                request.flags,
                POINT::default(),
                10,
                HWND::default(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
            .ok()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn measured_host_and_host2_vtables_have_exact_slots_and_widths() {
        let pointer = std::mem::size_of::<usize>();
        assert_eq!(std::mem::size_of::<HostVtbl>(), 5 * pointer);
        assert_eq!(std::mem::offset_of!(HostVtbl, show), 3 * pointer);
        assert_eq!(std::mem::offset_of!(HostVtbl, rect), 4 * pointer);
        assert_eq!(std::mem::size_of::<TabsVtbl>(), 4 * pointer);
        assert_eq!(std::mem::offset_of!(TabsVtbl, show), 3 * pointer);
        assert_eq!(std::mem::size_of::<POINT>(), 8);
        assert_eq!(std::mem::size_of::<RECT>(), 16);
    }
    #[test]
    fn host_facets_share_controlling_identity_and_reject_unknown_iids() {
        let _lock = TEST_SERVER.lock().unwrap();
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            let owner = Box::into_raw(Box::new(HostObject {
                vtable: &HOST_TABLE,
                refs: AtomicU32::new(1),
                native: OnceCell::new(),
                factory: None,
                tabs: TabsObject {
                    vtable: &TABS_TABLE,
                    owner: std::ptr::null_mut(),
                },
                _server: ServerPin::new(),
            }));
            (*owner).tabs.owner = owner;
            for from in [owner.cast(), (&mut (*owner).tabs as *mut TabsObject).cast()] {
                let query = if from == owner.cast() {
                    host_query
                } else {
                    tabs_query
                };
                for iid in [IUnknown::IID, HOST, HOST2] {
                    let mut facet = std::ptr::null_mut();
                    assert_eq!(query(from, &iid, &mut facet), S_OK);
                    let mut identity = std::ptr::null_mut();
                    let table = &**(facet as *const *const IUnknown_Vtbl);
                    assert_eq!(
                        (table.QueryInterface)(facet, &IUnknown::IID, &mut identity),
                        S_OK
                    );
                    assert_eq!(identity, owner.cast());
                    host_release(identity);
                    (table.Release)(facet);
                }
                let mut unknown = owner.cast();
                assert_eq!(
                    query(from, &GUID::from_u128(1), &mut unknown),
                    E_NOINTERFACE
                );
                assert!(unknown.is_null());
            }
            assert_eq!(host_release(owner.cast()), 0);
            CoUninitialize();
        }
    }
}
