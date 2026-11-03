use super::{clock, protocol::*};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::*,
        Security::{Authorization::*, *},
        Storage::FileSystem::*,
        System::{Pipes::*, Threading::*, IO::*},
        UI::WindowsAndMessaging::*,
    },
};

pub const READY: u32 = WM_APP + 59;
static LEDGER: OnceLock<Arc<Mutex<Ledger>>> = OnceLock::new();
static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());
static TARGET: AtomicUsize = AtomicUsize::new(0);
static SERVER: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessStamp {
    pub pid: u32,
    pub created: u64,
}

pub fn server_stamp() -> windows::core::Result<ProcessStamp> {
    process_stamp(SERVER.load(Ordering::Acquire))
}

pub fn listening() -> bool {
    let Ok(name) = name() else {
        return false;
    };
    unsafe {
        WaitNamedPipeW(PCWSTR(name.as_ptr()), NMPWAIT_NOWAIT).as_bool()
            || GetLastError() != ERROR_FILE_NOT_FOUND
    }
}

pub fn process_stamp(pid: u32) -> windows::core::Result<ProcessStamp> {
    unsafe {
        let process = Owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?);
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user)?;
        Ok(ProcessStamp {
            pid,
            created: ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64,
        })
    }
}

pub fn alive(stamp: ProcessStamp) -> Option<bool> {
    unsafe {
        let process = match OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            stamp.pid,
        ) {
            Ok(handle) => Owned(handle),
            Err(error) if error.code() == ERROR_INVALID_PARAMETER.to_hresult() => {
                return Some(false)
            }
            Err(_) => return None,
        };
        let current = process_stamp(stamp.pid).ok()?;
        if current.created != stamp.created {
            return Some(false);
        }
        match WaitForSingleObject(process.0, 0) {
            WAIT_OBJECT_0 => Some(false),
            WAIT_TIMEOUT => Some(true),
            _ => None,
        }
    }
}

pub struct Owned(pub HANDLE);
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub sid: String,
    pub session: u32,
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

pub fn identity(pid: u32) -> windows::core::Result<Identity> {
    unsafe {
        let process = Owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?);
        let mut token = HANDLE::default();
        OpenProcessToken(process.0, TOKEN_QUERY, &mut token)?;
        let token = Owned(token);
        token_identity(token.0)
    }
}

pub fn token_identity(token: HANDLE) -> windows::core::Result<Identity> {
    unsafe {
        let mut size = 0;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
        if size == 0 || size > 65_536 {
            return Err(windows::core::Error::from_hresult(E_ACCESSDENIED));
        }
        let mut data = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        GetTokenInformation(
            token,
            TokenUser,
            Some(data.as_mut_ptr().cast()),
            size,
            &mut size,
        )?;
        let user = &*(data.as_ptr() as *const TOKEN_USER);
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text)?;
        let sid = text.to_string();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        let mut session = 0u32;
        GetTokenInformation(
            token,
            TokenSessionId,
            Some((&mut session as *mut u32).cast()),
            4,
            &mut size,
        )?;
        Ok(Identity { sid: sid?, session })
    }
}

pub fn image(pid: u32) -> windows::core::Result<PathBuf> {
    unsafe {
        let process = Owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?);
        let mut text = vec![0u16; 32768];
        let mut size = text.len() as u32;
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(text.as_mut_ptr()),
            &mut size,
        )?;
        Ok(PathBuf::from(String::from_utf16_lossy(
            &text[..size as usize],
        )))
    }
}

pub fn navi_path() -> std::io::Result<PathBuf> {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Navi")
        .and_then(|key| key.get_value::<String, _>("ExplorerHostNaviPath"))
        .map(PathBuf::from)
        .or_else(|_| super::install::navi_path())
}

fn peer(pipe: HANDLE, server: bool) -> windows::core::Result<u32> {
    unsafe {
        let mut pid = 0;
        if server {
            GetNamedPipeServerProcessId(pipe, &mut pid)?;
        } else {
            GetNamedPipeClientProcessId(pipe, &mut pid)?;
        }
        if identity(pid)? != identity(std::process::id())? {
            return Err(windows::core::Error::from_hresult(E_ACCESSDENIED));
        }
        let expected = if server {
            navi_path().map_err(|_| windows::core::Error::from_hresult(E_FAIL))?
        } else {
            super::install::broker_path().map_err(|_| windows::core::Error::from_hresult(E_FAIL))?
        };
        let actual = image(pid)?;
        let bundled = server
            && super::install::navi_path().is_ok_and(|path| {
                actual
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&path.to_string_lossy())
            });
        if !bundled
            && !actual
                .to_string_lossy()
                .eq_ignore_ascii_case(&expected.to_string_lossy())
        {
            return Err(windows::core::Error::from_hresult(E_ACCESSDENIED));
        }
        Ok(pid)
    }
}

fn name() -> windows::core::Result<Vec<u16>> {
    let identity = identity(std::process::id())?;
    Ok(wide(&format!(
        r"\\.\pipe\Navi.ExplorerHost.v1.{}.{}",
        identity.sid, identity.session
    )))
}

unsafe fn transfer(
    pipe: HANDLE,
    buffer: &mut [u8],
    write: bool,
    deadline: u64,
) -> windows::core::Result<()> {
    let event = Owned(CreateEventW(None, true, false, None)?);
    let mut offset = 0;
    while offset < buffer.len() {
        if clock() >= deadline {
            return Err(windows::core::Error::from_hresult(HRESULT_TIMEOUT));
        }
        ResetEvent(event.0)?;
        let mut overlapped = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let result = if write {
            WriteFile(pipe, Some(&buffer[offset..]), None, Some(&mut overlapped))
        } else {
            ReadFile(
                pipe,
                Some(&mut buffer[offset..]),
                None,
                Some(&mut overlapped),
            )
        };
        if let Err(error) = result {
            if error.code() != ERROR_IO_PENDING.to_hresult() {
                return Err(error);
            }
            let wait = deadline.saturating_sub(clock()).min(u32::MAX as u64) as u32;
            if WaitForSingleObject(event.0, wait) != WAIT_OBJECT_0 {
                let _ = CancelIoEx(pipe, Some(&overlapped));
                let mut ignored = 0;
                let _ = GetOverlappedResult(pipe, &overlapped, &mut ignored, true);
                return Err(windows::core::Error::from_hresult(HRESULT_TIMEOUT));
            }
        }
        let mut size = 0;
        GetOverlappedResult(pipe, &overlapped, &mut size, true)?;
        if size == 0 {
            return Err(windows::core::Error::from_hresult(E_FAIL));
        }
        offset += size as usize;
    }
    Ok(())
}

const HRESULT_TIMEOUT: windows::core::HRESULT = windows::core::HRESULT(0x800705B4u32 as i32);

unsafe fn read_packet(pipe: HANDLE, deadline: u64) -> windows::core::Result<Vec<u8>> {
    let mut length = [0u8; 4];
    transfer(pipe, &mut length, false, deadline)?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > MAX_PACKET {
        return Err(windows::core::Error::from_hresult(E_INVALIDARG));
    }
    let mut data = vec![0; length];
    transfer(pipe, &mut data, false, deadline)?;
    Ok(data)
}

unsafe fn write_packet(pipe: HANDLE, data: &[u8], deadline: u64) -> windows::core::Result<()> {
    if data.len() > MAX_PACKET {
        return Err(windows::core::Error::from_hresult(E_INVALIDARG));
    }
    let mut packet = (data.len() as u32).to_le_bytes().to_vec();
    packet.extend_from_slice(data);
    transfer(pipe, &mut packet, true, deadline)
}

pub fn call(operation: Operation, deadline: u64) -> windows::core::Result<Reply> {
    unsafe {
        let name = name()?;
        let pipe = Owned(CreateFileW(
            PCWSTR(name.as_ptr()),
            FILE_READ_DATA.0 | FILE_WRITE_DATA.0,
            FILE_SHARE_MODE::default(),
            None,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            None,
        )?);
        let server = peer(pipe.0, true)?;
        let id = match &operation {
            Operation::Prepare(r) => &r.id,
            Operation::Commit(id) | Operation::Cancel(id) | Operation::Query(id) => id,
        }
        .clone();
        let data = serde_json::to_vec(&Message {
            version: VERSION,
            operation,
        })
        .map_err(|_| windows::core::Error::from_hresult(E_INVALIDARG))?;
        write_packet(pipe.0, &data, deadline)?;
        let reply: Reply = serde_json::from_slice(&read_packet(pipe.0, deadline)?)
            .map_err(|_| windows::core::Error::from_hresult(E_INVALIDARG))?;
        if reply.version != VERSION || reply.id != id {
            return Err(windows::core::Error::from_hresult(E_INVALIDARG));
        }
        SERVER.store(server, Ordering::Release);
        let mut ack = [1];
        let _ = transfer(pipe.0, &mut ack, true, deadline);
        Ok(reply)
    }
}

pub fn set_target(hwnd: HWND) {
    if !super::enabled() {
        return;
    }
    TARGET.store(hwnd.0 as usize, Ordering::Release);
}

pub fn start() {
    if !super::enabled() {
        return;
    }
    let ledger = Arc::new(Mutex::new(Ledger::default()));
    if LEDGER.set(ledger.clone()).is_err() {
        return;
    }
    let spawn = std::thread::Builder::new()
        .name("explorer-host-ipc".into())
        .spawn(move || {
            if let Err(error) = unsafe { serve(ledger) } {
                eprintln!("Explorer host IPC: {error}");
            }
        });
    if let Err(error) = spawn {
        eprintln!("Explorer host IPC worker: {error}");
    }
}

unsafe fn serve(ledger: Arc<Mutex<Ledger>>) -> windows::core::Result<()> {
    let sid = identity(std::process::id())?.sid;
    let sddl = wide(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})"));
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    ConvertStringSecurityDescriptorToSecurityDescriptorW(
        PCWSTR(sddl.as_ptr()),
        SDDL_REVISION_1,
        &mut descriptor,
        None,
    )?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let name = name()?;
    let raw = CreateNamedPipeW(
        PCWSTR(name.as_ptr()),
        PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
        1,
        MAX_PACKET as u32 + 4,
        MAX_PACKET as u32 + 4,
        0,
        Some(&attributes),
    );
    let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    if raw == INVALID_HANDLE_VALUE {
        return Err(windows::core::Error::from_win32());
    }
    let pipe = Owned(raw);
    let event = Owned(CreateEventW(None, true, false, None)?);
    loop {
        ResetEvent(event.0)?;
        let mut overlapped = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        match ConnectNamedPipe(pipe.0, Some(&mut overlapped)) {
            Ok(()) => {}
            Err(error) if error.code() == ERROR_PIPE_CONNECTED.to_hresult() => {}
            Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => {
                if WaitForSingleObject(event.0, INFINITE) != WAIT_OBJECT_0 {
                    return Err(windows::core::Error::from_win32());
                }
                let mut size = 0;
                GetOverlappedResult(pipe.0, &overlapped, &mut size, false)?;
            }
            Err(error) => return Err(error),
        }
        let result = (|| -> windows::core::Result<()> {
            peer(pipe.0, false)?;
            if !super::enabled() {
                return Err(windows::core::Error::from_hresult(E_ACCESSDENIED));
            }
            let deadline = clock().saturating_add(150);
            let data = read_packet(pipe.0, deadline)?;
            let message = Message::decode(&data)
                .ok_or_else(|| windows::core::Error::from_hresult(E_INVALIDARG))?;
            let commit = matches!(message.operation, Operation::Commit(_));
            let reply = ledger
                .lock()
                .map_err(|_| windows::core::Error::from_hresult(E_FAIL))?
                .receive(message, clock());
            if commit && reply.state == State::Queued {
                let mut pending = PENDING
                    .lock()
                    .map_err(|_| windows::core::Error::from_hresult(E_FAIL))?;
                if !pending.contains(&reply.id) {
                    pending.push(reply.id.clone());
                }
                let hwnd = HWND(TARGET.load(Ordering::Acquire) as _);
                if hwnd.is_invalid()
                    || PostMessageW(Some(hwnd), READY, WPARAM(0), LPARAM(0)).is_err()
                {
                    ledger
                        .lock()
                        .map_err(|_| windows::core::Error::from_hresult(E_FAIL))?
                        .receive(
                            Message {
                                version: VERSION,
                                operation: Operation::Cancel(reply.id.clone()),
                            },
                            clock(),
                        );
                    pending.retain(|id| id != &reply.id);
                    return Err(windows::core::Error::from_hresult(E_FAIL));
                }
            }
            write_packet(
                pipe.0,
                &serde_json::to_vec(&reply)
                    .map_err(|_| windows::core::Error::from_hresult(E_FAIL))?,
                deadline,
            )?;
            let mut ack = [0];
            transfer(pipe.0, &mut ack, false, deadline)
        })();
        let _ = result;
        let _ = DisconnectNamedPipe(pipe.0);
    }
}

pub fn take() -> Vec<String> {
    PENDING
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .unwrap_or_default()
}

pub fn begin(id: &str) -> Option<Request> {
    let mut ledger = LEDGER.get()?.lock().ok()?;
    if !super::enabled() {
        ledger.receive(
            Message {
                version: VERSION,
                operation: Operation::Cancel(id.into()),
            },
            clock(),
        );
        return None;
    }
    ledger.begin(id, clock())
}

pub fn apply(id: &str) -> bool {
    LEDGER
        .get()
        .and_then(|ledger| ledger.lock().ok())
        .is_some_and(|mut ledger| {
            if !super::enabled() {
                ledger.receive(
                    Message {
                        version: VERSION,
                        operation: Operation::Cancel(id.into()),
                    },
                    clock(),
                );
                return false;
            }
            ledger.apply(id, clock())
        })
}

pub fn retarget(hwnd: HWND, next: HWND) {
    let _ = TARGET.compare_exchange(
        hwnd.0 as usize,
        next.0 as usize,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

pub fn finish(id: &str, painted: bool) {
    if let Some(ledger) = LEDGER.get() {
        if let Ok(mut ledger) = ledger.lock() {
            ledger.finish(id, painted);
        }
    }
}

pub fn cancel(id: &str) {
    if let Some(ledger) = LEDGER.get() {
        if let Ok(mut ledger) = ledger.lock() {
            ledger.cancel_unpresented(id);
        }
    }
}

pub fn retry_pause() {
    std::thread::sleep(Duration::from_millis(5));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_identity_comes_from_process_token_not_payload() {
        let actual = identity(std::process::id()).unwrap();
        assert!(actual.sid.starts_with("S-1-5-"));
        assert_ne!(actual.session, 0);
        assert!(image(std::process::id()).unwrap().is_absolute());
        assert!(identity(u32::MAX).is_err());
    }
    #[test]
    fn pid_reuse_does_not_keep_a_previous_ui_incarnation_alive() {
        let stamp = process_stamp(std::process::id()).unwrap();
        assert_eq!(alive(stamp), Some(true));
        assert_eq!(
            alive(ProcessStamp {
                created: stamp.created + 1,
                ..stamp
            }),
            Some(false)
        );
        assert_eq!(
            alive(ProcessStamp {
                pid: u32::MAX,
                created: 1
            }),
            Some(false)
        );
    }
}
