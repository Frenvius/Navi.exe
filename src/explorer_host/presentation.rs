use super::{
    clock,
    protocol::Request,
    transport::{self, Owned, ProcessStamp},
};
use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::io::AsRawHandle,
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
};
use windows::{
    core::{BOOL, PCWSTR},
    Win32::{
        Foundation::*,
        Graphics::Dwm::*,
        System::{Memory::*, Threading::*},
        UI::WindowsAndMessaging::*,
    },
};

const MAGIC: u64 = 0x4e41564948535432;
const PENDING: u32 = 0;
const PUBLISHING: u32 = 1;
const NAVI: u32 = 2;
const NATIVE: u32 = 3;
const NATIVE_DONE: u32 = 4;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    Pending,
    Publishing,
    Navi,
    Native,
    NativeDone,
}
#[repr(C)]
struct Record {
    magic: AtomicU64,
    id_low: AtomicU64,
    id_high: AtomicU64,
    deadline: AtomicU64,
    state: AtomicU32,
    ui_pid: AtomicU32,
    ui_created: AtomicU64,
    window: AtomicU64,
    native_pid: AtomicU32,
    owner_pid: AtomicU32,
    native_created: AtomicU64,
    owner_created: AtomicU64,
    owner_deadline: AtomicU64,
    native_started: AtomicU64,
    native_window: AtomicU64,
    visible_tick: AtomicU64,
    native_reconciled: AtomicU64,
}
struct Mapping {
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    _mapping: Owned,
    _file: File,
    ready: Owned,
}
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
        }
    }
}
#[derive(Clone)]
pub struct Gate(Arc<Mapping>);
fn path(request: &Request) -> io::Result<PathBuf> {
    if request.id.len() != 32 || !request.id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid presentation ID",
        ));
    }
    #[cfg(not(test))]
    let root = PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or_else(|| io::Error::other("LOCALAPPDATA"))?,
    )
    .join("Navi/explorer-host/admissions");
    #[cfg(test)]
    let root =
        std::env::temp_dir().join(format!("NaviHostPresentationTests-{}", std::process::id()));
    Ok(root.join(format!("{}.presentation", request.id)))
}
fn error(error: windows::core::Error) -> io::Error {
    io::Error::other(error)
}
impl Gate {
    pub fn create(request: &Request) -> io::Result<Self> {
        Self::map(request, true)
    }
    pub fn open(request: &Request) -> io::Result<Self> {
        Self::map(request, false)
    }
    fn map(request: &Request, create: bool) -> io::Result<Self> {
        let path = path(request)?;
        super::security::no_reparse(&path)?;
        if create {
            std::fs::create_dir_all(path.parent().unwrap())?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(create)
            .open(path)?;
        if create {
            file.set_len(std::mem::size_of::<Record>() as u64)?;
        } else if file.metadata()?.len() != std::mem::size_of::<Record>() as u64 {
            return Err(io::Error::other("Presentation record size"));
        }
        unsafe {
            let mapping = Owned(
                CreateFileMappingW(
                    HANDLE(file.as_raw_handle()),
                    None,
                    PAGE_READWRITE,
                    0,
                    0,
                    PCWSTR::null(),
                )
                .map_err(error)?,
            );
            let view = MapViewOfFile(
                mapping.0,
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                std::mem::size_of::<Record>(),
            );
            if view.Value.is_null() {
                return Err(io::Error::last_os_error());
            }
            let ready = match CreateEventW(None, true, false, None) {
                Ok(event) => Owned(event),
                Err(cause) => {
                    let _ = UnmapViewOfFile(view);
                    return Err(error(cause));
                }
            };
            let gate = Self(Arc::new(Mapping {
                view,
                _mapping: mapping,
                _file: file,
                ready,
            }));
            let id = u128::from_str_radix(&request.id, 16).map_err(io::Error::other)?;
            if create {
                gate.record().id_low.store(id as u64, Ordering::Relaxed);
                gate.record()
                    .id_high
                    .store((id >> 64) as u64, Ordering::Relaxed);
                gate.record()
                    .deadline
                    .store(request.deadline, Ordering::Relaxed);
                gate.record().magic.store(MAGIC, Ordering::Release);
            }
            if gate.record().magic.load(Ordering::Acquire) != MAGIC
                || gate.record().id_low.load(Ordering::Relaxed) != id as u64
                || gate.record().id_high.load(Ordering::Relaxed) != (id >> 64) as u64
                || gate.record().deadline.load(Ordering::Relaxed) != request.deadline
            {
                return Err(io::Error::other("Presentation identity mismatch"));
            }
            Ok(gate)
        }
    }
    fn record(&self) -> &Record {
        unsafe { &*self.0.view.Value.cast::<Record>() }
    }
    pub fn destination(&self) -> Destination {
        match self.record().state.load(Ordering::Acquire) {
            PENDING => Destination::Pending,
            PUBLISHING => Destination::Publishing,
            NAVI => Destination::Navi,
            NATIVE_DONE => Destination::NativeDone,
            _ => Destination::Native,
        }
    }
    pub fn cancel(&self) -> Destination {
        let _ = self.record().state.compare_exchange(
            PENDING,
            NATIVE,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.destination()
    }
    pub fn ui(&self) -> Option<ProcessStamp> {
        let pid = self.record().ui_pid.load(Ordering::Acquire);
        (pid != 0).then(|| ProcessStamp {
            pid,
            created: self.record().ui_created.load(Ordering::Relaxed),
        })
    }
    pub fn reclaim_dead_ui(&self) -> bool {
        if self
            .ui()
            .is_some_and(|stamp| transport::alive(stamp) == Some(false))
        {
            let state = self.record().state.load(Ordering::Acquire);
            if matches!(state, NAVI | PUBLISHING) {
                return self
                    .record()
                    .state
                    .compare_exchange(state, NATIVE, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok();
            }
        }
        false
    }
    pub fn visible_tick(&self) -> u64 {
        self.record().visible_tick.load(Ordering::Acquire)
    }
    pub fn window(&self) -> usize {
        self.record().window.load(Ordering::Acquire) as usize
    }
    pub fn native_started(&self) -> u64 {
        self.record().native_started.load(Ordering::Acquire)
    }
    pub fn calling_native(&self) {
        let _ = self.record().native_started.try_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |previous| Some(clock().max(previous.saturating_add(1))),
        );
    }
    pub fn native_reconciled(&self) -> u64 {
        self.record().native_reconciled.load(Ordering::Acquire)
    }
    pub fn mark_native_reconciled(&self, attempt: u64) {
        self.record()
            .native_reconciled
            .store(attempt, Ordering::Release);
    }
    pub fn native_process(&self) -> Option<ProcessStamp> {
        let pid = self.record().native_pid.load(Ordering::Acquire);
        (pid != 0).then(|| ProcessStamp {
            pid,
            created: self.record().native_created.load(Ordering::Relaxed),
        })
    }
    pub fn set_native_process(&self, stamp: ProcessStamp) {
        self.record()
            .native_created
            .store(stamp.created, Ordering::Relaxed);
        self.record().native_pid.store(stamp.pid, Ordering::Release);
    }
    pub fn native_accepted(&self) {
        self.native_done(0);
    }
    pub fn native_done(&self, hwnd: usize) {
        self.record()
            .native_window
            .store(hwnd as u64, Ordering::Relaxed);
        self.record().state.store(NATIVE_DONE, Ordering::Release);
    }
    pub fn owner(&self) -> Option<(ProcessStamp, u64)> {
        let pid = self.record().owner_pid.load(Ordering::Acquire);
        (pid != 0).then(|| {
            (
                ProcessStamp {
                    pid,
                    created: self.record().owner_created.load(Ordering::Relaxed),
                },
                self.record().owner_deadline.load(Ordering::Acquire),
            )
        })
    }
    pub fn set_owner(&self, deadline: u64) -> io::Result<()> {
        let stamp = transport::process_stamp(std::process::id()).map_err(error)?;
        self.record()
            .owner_created
            .store(stamp.created, Ordering::Relaxed);
        self.record()
            .owner_deadline
            .store(deadline, Ordering::Relaxed);
        self.record().owner_pid.store(stamp.pid, Ordering::Release);
        Ok(())
    }
    pub fn ready(&self) {
        unsafe {
            let _ = SetEvent(self.0.ready.0);
        }
    }
    pub fn stage(&self, hwnd: HWND) -> io::Result<bool> {
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        if pid != std::process::id()
            || self.destination() != Destination::Pending
            || clock() >= self.record().deadline.load(Ordering::Acquire)
        {
            self.cancel();
            return Ok(false);
        }
        let stamp = transport::process_stamp(pid).map_err(error)?;
        self.record()
            .ui_created
            .store(stamp.created, Ordering::Relaxed);
        self.record()
            .window
            .store(hwnd.0 as usize as u64, Ordering::Relaxed);
        self.record().ui_pid.store(pid, Ordering::Release);
        cloak(hwnd, true)?;
        let gate = self.clone();
        let target = hwnd.0 as usize;
        let spawned = std::thread::Builder::new()
            .name("explorer-host-present".into())
            .spawn(move || {
                let deadline = gate.record().deadline.load(Ordering::Acquire);
                let ready = unsafe {
                    WaitForSingleObject(
                        gate.0.ready.0,
                        deadline.saturating_sub(clock()).min(u32::MAX as u64) as u32,
                    )
                } == WAIT_OBJECT_0;
                if ready
                    && clock() < deadline
                    && gate
                        .record()
                        .state
                        .compare_exchange(PENDING, PUBLISHING, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    let window = HWND(target as _);
                    if cloak(window, false)
                        .and_then(|_| unsafe { DwmFlush().map_err(error) })
                        .is_ok()
                    {
                        gate.record().visible_tick.store(clock(), Ordering::Relaxed);
                        gate.record().state.store(NAVI, Ordering::Release);
                    } else {
                        let _ = cloak(window, true);
                        gate.record().state.store(NATIVE, Ordering::Release);
                    }
                } else {
                    gate.cancel();
                }
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(target as _)),
                        transport::READY,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            });
        if let Err(cause) = spawned {
            self.cancel();
            return Err(cause);
        }
        Ok(true)
    }
}
pub fn cloak(hwnd: HWND, hidden: bool) -> io::Result<()> {
    let value = BOOL::from(hidden);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            (&value as *const BOOL).cast(),
            std::mem::size_of::<BOOL>() as u32,
        )
        .map_err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Request {
        Request {
            id: format!(
                "{:032x}",
                unsafe { windows::Win32::System::Com::CoCreateGuid().unwrap() }.to_u128()
            ),
            deadline: clock() + 350,
            path: r"C:\fixture".into(),
            pidl: vec![0, 0],
            selection: vec![],
            flags: 0,
            new_tab: true,
            new_window: false,
        }
    }
    #[test]
    fn cancellation_survives_reopen_and_blocks_late_publish() {
        let r = request();
        let gate = Gate::create(&r).unwrap();
        assert_eq!(gate.cancel(), Destination::Native);
        let reopened = Gate::open(&r).unwrap();
        assert_eq!(reopened.destination(), Destination::Native);
        assert!(reopened
            .record()
            .state
            .compare_exchange(PENDING, PUBLISHING, Ordering::AcqRel, Ordering::Acquire)
            .is_err());
        drop(reopened);
        drop(gate);
        std::fs::remove_file(path(&r).unwrap()).unwrap();
    }
    #[test]
    fn a_lost_pipe_ack_does_not_authorize_a_second_destination() {
        let r = request();
        let gate = Gate::create(&r).unwrap();
        gate.record().state.store(NAVI, Ordering::Release);
        assert_eq!(gate.cancel(), Destination::Navi);
        gate.native_done(123);
        assert_eq!(gate.cancel(), Destination::NativeDone);
        drop(gate);
        std::fs::remove_file(path(&r).unwrap()).unwrap();
    }
    #[test]
    fn concurrent_timeout_and_publish_choose_one_destination() {
        let r = request();
        let gate = Gate::create(&r).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(9));
        let mut threads = Vec::new();
        for index in 0..8 {
            let gate = gate.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                if index % 2 == 0 {
                    gate.cancel();
                } else if gate
                    .record()
                    .state
                    .compare_exchange(PENDING, PUBLISHING, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    gate.record().state.store(NAVI, Ordering::Release);
                }
            }));
        }
        barrier.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        assert!(matches!(
            gate.destination(),
            Destination::Navi | Destination::Native
        ));
        let again = Gate::open(&r).unwrap();
        assert_eq!(again.destination(), gate.destination());
        drop(again);
        drop(gate);
        std::fs::remove_file(path(&r).unwrap()).unwrap();
    }

    #[test]
    fn a_reconciled_native_attempt_does_not_hide_a_new_dispatch() {
        let r = request();
        let gate = Gate::create(&r).unwrap();
        gate.calling_native();
        let first = gate.native_started();
        gate.mark_native_reconciled(first);
        gate.calling_native();
        assert!(gate.native_started() > first);
        assert_ne!(gate.native_started(), gate.native_reconciled());
        drop(gate);
        std::fs::remove_file(path(&r).unwrap()).unwrap();
    }
}
