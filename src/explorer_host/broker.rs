use crate::explorer_host::{clock, protocol::*, transport};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Instant,
};
use windows::{
    core::PCWSTR,
    Win32::{
        System::Com::*,
        UI::Shell::{Common::ITEMIDLIST, *},
    },
};

#[path = "com.rs"]
mod com;
#[path = "recovery.rs"]
mod recovery;

fn lab() -> bool {
    LAB.get().copied().unwrap_or(false)
}
fn fault_name() -> Option<&'static str> {
    FAULT.get().map(String::as_str)
}
fn broker_fault(name: &str) -> bool {
    fault_name() == Some(name)
}

static LAB: OnceLock<bool> = OnceLock::new();
static FAULT: OnceLock<String> = OnceLock::new();
static FAULT_ONCE: OnceLock<PathBuf> = OnceLock::new();

pub fn trace(event: &str, data: serde_json::Value) {
    if !LAB.get().copied().unwrap_or(false) {
        return;
    }
    let Some(root) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
        return;
    };
    let root = root.join("Navi/explorer-host");
    if fs::create_dir_all(&root).is_err() {
        return;
    }
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("events.jsonl"))
    {
        let event =
            serde_json::json!({"pid":std::process::id(),"tick":clock(),"event":event,"data":data});
        let _ = writeln!(file, "{event}");
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Admission {
    version: u32,
    identity: String,
    session: u32,
    #[serde(default)]
    native_barrier: bool,
    request: Request,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Outcome {
    pub state: State,
    pub elapsed_ms: u128,
    pub fallback_allowed: bool,
}

pub fn deliver(request: Request) -> Outcome {
    let Ok(_gate) = crate::explorer_host::presentation::Gate::create(&request) else {
        return Outcome {
            state: State::Rejected,
            elapsed_ms: 0,
            fallback_allowed: true,
        };
    };
    deliver_inner(request, None)
}

struct Bootstrap(transport::Owned);

impl Bootstrap {
    fn claim(name: &str, sid: &str) -> std::io::Result<Option<Self>> {
        use windows::Win32::{Foundation::*, System::Threading::*};
        let descriptor = crate::explorer_host::security::Descriptor::new(&format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;{sid})"
        ))?;
        let name = crate::explorer_host::security::wide(name);
        unsafe {
            let mutex = transport::Owned(
                CreateMutexW(Some(&descriptor.attributes()), false, PCWSTR(name.as_ptr()))
                    .map_err(std::io::Error::other)?,
            );
            match WaitForSingleObject(mutex.0, 0) {
                WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(Self(mutex))),
                WAIT_TIMEOUT => Ok(None),
                _ => Err(std::io::Error::last_os_error()),
            }
        }
    }
}

impl Drop for Bootstrap {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::System::Threading::ReleaseMutex(self.0 .0);
        }
    }
}

fn start_navi(request: &Request) -> Option<Bootstrap> {
    let identity = transport::identity(std::process::id()).ok()?;
    let guard = Bootstrap::claim(
        &format!(
            "Local\\Navi.ExplorerHost.Bootstrap.{}.{}",
            identity.sid, identity.session
        ),
        &identity.sid,
    )
    .ok()??;
    if transport::listening() {
        return Some(guard);
    }
    if let Ok(path) = transport::navi_path() {
        let _ = std::process::Command::new(path)
            .arg("--explorer-host-bootstrap")
            .arg(&request.path)
            .spawn();
    }
    Some(guard)
}
fn deliver_inner(request: Request, commit_intent: Option<&Path>) -> Outcome {
    let started = Instant::now();
    let id = request.id.clone();
    let deadline = request.deadline;
    let outcome = |state, fallback_allowed| Outcome {
        state,
        elapsed_ms: started.elapsed().as_millis(),
        fallback_allowed,
    };
    let call = |operation| transport::call(operation, (clock() + 50).min(deadline));
    let mut prepared = false;
    let mut bootstrap = false;
    let mut bootstrap_guard = None;
    while clock() < deadline {
        match call(Operation::Prepare(request.clone())) {
            Ok(reply) if matches!(reply.state, State::Prepared | State::Queued) => {
                prepared = true;
                break;
            }
            Ok(reply)
                if matches!(
                    reply.state,
                    State::Completed
                        | State::Executing
                        | State::Applying
                        | State::Ambiguous
                        | State::Conflict
                ) =>
            {
                return outcome(reply.state, false);
            }
            Ok(reply)
                if matches!(
                    reply.state,
                    State::Cancelled | State::Expired | State::Rejected
                ) =>
            {
                return outcome(reply.state, true);
            }
            Err(error) if error.code() == windows::Win32::Foundation::E_ACCESSDENIED => {
                return outcome(State::Rejected, true);
            }
            Err(error)
                if !bootstrap
                    && error.code()
                        == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND.to_hresult() =>
            {
                bootstrap = true;
                bootstrap_guard = start_navi(&request);
                transport::retry_pause();
            }
            _ => transport::retry_pause(),
        }
    }
    if !prepared {
        return outcome(State::Unknown, true);
    }
    drop(bootstrap_guard);
    let stamp = match transport::server_stamp() {
        Ok(stamp) => stamp,
        Err(_) => return outcome(State::Cancelled, true),
    };
    if FAULT.get().is_some_and(|fault| fault == "before-commit") {
        let reply = transport::call(Operation::Cancel(id), clock() + 100);
        return outcome(reply.map(|r| r.state).unwrap_or(State::Unknown), true);
    }
    if let Some(path) = commit_intent {
        if write_new(path, &serde_json::to_vec(&stamp).unwrap_or_default()).is_err() {
            let _ = transport::call(Operation::Cancel(id), clock() + 100);
            return outcome(State::Rejected, true);
        }
    }
    while clock() < deadline {
        match call(Operation::Commit(id.clone())) {
            Ok(reply)
                if matches!(
                    reply.state,
                    State::Queued
                        | State::Executing
                        | State::Applying
                        | State::Completed
                        | State::Ambiguous
                ) =>
            {
                break
            }
            Ok(reply)
                if matches!(
                    reply.state,
                    State::Cancelled | State::Expired | State::Rejected
                ) =>
            {
                return outcome(reply.state, true);
            }
            _ => transport::retry_pause(),
        }
    }
    if FAULT
        .get()
        .is_some_and(|fault| fault == "worker-crash-after-commit")
    {
        std::process::exit(23);
    }
    if FAULT
        .get()
        .is_some_and(|fault| fault == "worker-hang-after-commit")
    {
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
    let queued_until = clock().saturating_add(75).min(deadline);
    while clock() < deadline {
        if transport::alive(stamp) == Some(false) {
            return outcome(State::Cancelled, true);
        }
        match call(Operation::Query(id.clone())) {
            Ok(reply) if reply.state == State::Queued && clock() >= queued_until => {
                let reply = transport::call(Operation::Cancel(id.clone()), deadline);
                return outcome(
                    reply.as_ref().map_or(State::Unknown, |reply| reply.state),
                    reply.is_ok_and(|reply| {
                        matches!(
                            reply.state,
                            State::Cancelled | State::Expired | State::Rejected
                        )
                    }),
                );
            }
            Ok(reply)
                if matches!(
                    reply.state,
                    State::Completed
                        | State::Cancelled
                        | State::Expired
                        | State::Rejected
                        | State::Ambiguous
                ) =>
            {
                return outcome(
                    reply.state,
                    matches!(
                        reply.state,
                        State::Cancelled | State::Expired | State::Rejected
                    ),
                );
            }
            _ => transport::retry_pause(),
        }
    }
    let reconciliation_deadline = clock() + 150;
    while clock() < reconciliation_deadline {
        if let Ok(reply) = transport::call(Operation::Cancel(id.clone()), reconciliation_deadline) {
            return outcome(
                reply.state,
                matches!(
                    reply.state,
                    State::Cancelled | State::Expired | State::Rejected
                ),
            );
        }
        transport::retry_pause();
    }
    outcome(State::Ambiguous, transport::alive(stamp) == Some(false))
}

fn write_new(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(data)?;
    file.sync_all()
}

unsafe fn request(pidl: *const ITEMIDLIST, flags: i32) -> windows::core::Result<Request> {
    if pidl.is_null() {
        return Err(windows::core::Error::from_hresult(
            windows::Win32::Foundation::E_INVALIDARG,
        ));
    }
    let size = ILGetSize(Some(pidl)) as usize;
    if size < 2 || size > 65_536 {
        return Err(windows::core::Error::from_hresult(
            windows::Win32::Foundation::E_INVALIDARG,
        ));
    }
    let name = SHGetNameFromIDList(pidl, SIGDN_FILESYSPATH)?;
    let text = name.to_string();
    CoTaskMemFree(Some(name.0.cast()));
    let text = text?;
    let (path, selection) = if flags == 4 {
        (
            Path::new(&text)
                .parent()
                .ok_or_else(windows::core::Error::from_win32)?
                .to_string_lossy()
                .into_owned(),
            vec![text],
        )
    } else {
        (text, Vec::new())
    };
    let id = format!("{:032x}", CoCreateGuid()?.to_u128());
    let new_window = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Navi")
        .and_then(|key| key.get_value::<u32, _>("ExternalOpenDestination"))
        .is_ok_and(|value| value == 1);
    Ok(Request {
        id,
        deadline: clock()
            + if transport::listening() {
                if new_window {
                    500
                } else {
                    400
                }
            } else {
                LEASE_MS
            },
        pidl: std::slice::from_raw_parts(pidl.cast(), size).to_vec(),
        path,
        selection,
        flags,
        new_tab: !new_window,
        new_window,
    })
}

pub unsafe fn admit(pidl: *const ITEMIDLIST, flags: i32) -> bool {
    if !crate::explorer_host::enabled()
        || crate::explorer_host::install::draining()
        || !crate::explorer_host::version::verified()
    {
        return false;
    }
    let Ok(request) = request(pidl, flags) else {
        return false;
    };
    if request.path.starts_with(r"\\")
        || !transport::navi_path().is_ok_and(|path| path.is_file())
        || !std::path::Path::new(&request.path).is_dir()
    {
        return false;
    }
    let Ok(_gate) = crate::explorer_host::presentation::Gate::create(&request) else {
        return false;
    };
    trace(
        "admit",
        serde_json::json!({"id":request.id,"path":request.path}),
    );
    let Ok(identity) = transport::identity(std::process::id()) else {
        return false;
    };
    let Some(root) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
        return false;
    };
    let root = root.join("Navi/explorer-host/admissions");
    if fs::create_dir_all(&root).is_err() {
        return false;
    }
    let path = root.join(format!("{}.json", request.id));
    let admission = Admission {
        version: VERSION,
        identity: identity.sid,
        session: identity.session,
        native_barrier: true,
        request,
    };
    let Ok(bytes) = serde_json::to_vec(&admission) else {
        return false;
    };
    if bytes.len() > MAX_PACKET {
        return false;
    }
    if write_new(&path, &bytes).is_err() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let fault = FAULT.get().filter(|_| {
        FAULT_ONCE
            .get()
            .is_none_or(|path| write_new(path, admission.request.id.as_bytes()).is_ok())
    });
    let command = |mode| {
        let mut command = std::process::Command::new(&exe);
        command.arg(mode).arg(&path);
        if lab() {
            command.arg("--lab");
        }
        if let Some(fault) = fault {
            command.arg("--fault").arg(fault);
        }
        command
    };
    let _ = command("--native").spawn();
    match command("--guard").spawn() {
        Ok(_) => {
            let _ = command("--guard").spawn();
            if fault.is_some_and(|fault| fault == "broker-crash-after-admission") {
                std::process::exit(23);
            }
            if fault.is_some_and(|fault| fault == "broker-hang-after-admission") {
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
            true
        }
        Err(_) => false,
    }
}

fn read_admission(path: &Path) -> Result<Admission, Box<dyn std::error::Error>> {
    let identity = transport::identity(std::process::id())?;
    let root = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA")?)
        .join("Navi/explorer-host/admissions");
    if path.parent() != Some(root.as_path())
        || fs::symlink_metadata(path)?.file_type().is_symlink()
        || fs::metadata(path)?.len() > MAX_PACKET as u64
    {
        return Err("Invalid admission path".into());
    }
    let admission: Admission = serde_json::from_slice(&fs::read(path)?)?;
    if admission.version != VERSION
        || admission.identity != identity.sid
        || admission.session != identity.session
        || path.file_stem().and_then(|s| s.to_str()) != Some(&admission.request.id)
    {
        return Err("Invalid admission owner".into());
    }
    Ok(admission)
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let option = |key: &str| {
        args.iter()
            .position(|v| v == key)
            .and_then(|i| args.get(i + 1))
    };
    LAB.set(args.iter().any(|v| v == "--lab")).ok();
    if LAB.get().copied().unwrap_or(false) {
        if let Some(path) = option("--lab-state-dir") {
            std::env::set_var("NAVI_ROUTE1_STATE", path);
        }
        if let Some(fault) = option("--fault") {
            FAULT.set(fault.clone()).ok();
        }
        if let Some(path) = option("--fault-once") {
            FAULT_ONCE.set(PathBuf::from(path)).ok();
        }
    }
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        if args
            .iter()
            .any(|value| matches!(value.as_str(), "--install" | "--repair" | "--uninstall"))
        {
            let result = if args.iter().any(|value| value == "--install") {
                crate::explorer_host::install::install(Path::new(
                    option("--source").ok_or("Missing installation source")?,
                ))
            } else if args.iter().any(|value| value == "--repair") {
                crate::explorer_host::install::repair()
            } else {
                crate::explorer_host::install::uninstall()
            };
            if let Some(out) = option("--out") {
                fs::write(
                    out,
                    serde_json::to_vec_pretty(
                        &serde_json::json!({"ok":result.is_ok(),"error":result.as_ref().err().map(ToString::to_string)}),
                    )?,
                )?;
            }
            return result.map_err(Into::into);
        }
        if let Some(path) = option("--guard") {
            return recovery::supervise(Path::new(path));
        }
        if let Some(path) = option("--native") {
            return recovery::native_worker(Path::new(path));
        }
        if let Some(path) = option("--worker") {
            return recovery::worker(Path::new(path));
        }
        if let Some(path) = option("--send") {
            let input: Vec<_> = path.encode_utf16().chain([0]).collect();
            let mut pidl = std::ptr::null_mut();
            unsafe {
                SHParseDisplayName(PCWSTR(input.as_ptr()), None, &mut pidl, 0, None)?;
                let value = request(
                    pidl,
                    if args.iter().any(|v| v == "--select") {
                        4
                    } else {
                        0
                    },
                );
                CoTaskMemFree(Some(pidl.cast()));
                let request = value?;
                let result = deliver(request);
                if let Some(path) = option("--out") {
                    write_new(Path::new(path), &serde_json::to_vec_pretty(&result)?)?;
                }
            }
            return Ok(());
        }
        if args.iter().any(|v| v == "--serve") {
            let seconds: u64 = option("--seconds")
                .map(|s| s.parse())
                .transpose()?
                .unwrap_or(0);
            return com::serve(seconds.min(600), option("--stop-event").map(String::as_str));
        }
        Err("Missing Explorer host operation".into())
    })();
    unsafe {
        CoUninitialize();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simultaneous_cold_requests_elect_one_bootstrap_and_release_it_after_admission() {
        let identity = transport::identity(std::process::id()).unwrap();
        let name = format!(
            "Local\\Navi.ExplorerHost.Bootstrap.Test.{}.{}",
            std::process::id(),
            clock()
        );
        let guard = Bootstrap::claim(&name, &identity.sid).unwrap().unwrap();
        let other_name = name.clone();
        let other_sid = identity.sid.clone();
        assert!(std::thread::spawn(move || {
            Bootstrap::claim(&other_name, &other_sid).unwrap().is_none()
        })
        .join()
        .unwrap());
        drop(guard);
        assert!(std::thread::spawn(move || {
            Bootstrap::claim(&name, &identity.sid).unwrap().is_some()
        })
        .join()
        .unwrap());
    }
}
