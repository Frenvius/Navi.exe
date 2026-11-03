use crate::explorer_host::{
    clock,
    presentation::{Destination, Gate},
    protocol::Request,
    transport::{self, Owned, ProcessStamp},
};
use std::{fs, io, path::Path, process::Command, time::Duration};
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        Foundation::*,
        System::{Com::*, Threading::*, Variant::VARIANT},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
};

fn error(error: windows::core::Error) -> io::Error {
    io::Error::other(error)
}
fn once(path: &Path, label: &str) -> bool {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path.with_extension(format!("fault-{label}")))
        .is_ok()
}
fn fault(path: &Path, label: &str) -> bool {
    super::broker_fault(label) && once(path, label)
}
pub(super) fn command(mode: &str, path: &Path) -> io::Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command.arg(mode).arg(path);
    if super::lab() {
        command.arg("--lab");
    }
    if let Some(fault) = super::fault_name() {
        command.arg("--fault").arg(fault);
    }
    Ok(command)
}
fn terminate(stamp: ProcessStamp) -> io::Result<()> {
    if transport::alive(stamp) != Some(true) {
        return Ok(());
    }
    if transport::identity(stamp.pid).map_err(error)?
        != transport::identity(std::process::id()).map_err(error)?
        || transport::image(stamp.pid).map_err(error)? != std::env::current_exe()?
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Recovery process identity mismatch",
        ));
    }
    unsafe {
        let process = Owned(
            OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, stamp.pid)
                .map_err(error)?,
        );
        if transport::process_stamp(stamp.pid).map_err(error)?.created != stamp.created {
            return Ok(());
        }
        TerminateProcess(process.0, 23).map_err(error)?;
        if WaitForSingleObject(process.0, 200) != WAIT_OBJECT_0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Recovery process termination did not complete",
            ));
        }
    }
    Ok(())
}
struct Lock(Owned);
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0 .0);
        }
    }
}
fn acquire(request: &Request, gate: &Gate) -> io::Result<Option<Lock>> {
    let identity = transport::identity(std::process::id()).map_err(error)?;
    let name = crate::explorer_host::security::wide(&format!(
        r"Local\Navi.ExplorerHost.Recovery.{}.{}.{}",
        identity.sid, identity.session, request.id
    ));
    let descriptor = crate::explorer_host::security::Descriptor::new(&format!(
        "D:P(A;;GA;;;SY)(A;;GA;;;{})",
        identity.sid
    ))?;
    let mutex = unsafe {
        Owned(
            CreateMutexW(Some(&descriptor.attributes()), false, PCWSTR(name.as_ptr()))
                .map_err(error)?,
        )
    };
    loop {
        if matches!(gate.destination(), Destination::NativeDone)
            || (gate.destination() == Destination::Navi
                && gate
                    .ui()
                    .is_some_and(|stamp| transport::alive(stamp) == Some(true)))
        {
            return Ok(None);
        }
        match unsafe { WaitForSingleObject(mutex.0, 20) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => return Ok(Some(Lock(mutex))),
            WAIT_TIMEOUT => {}
            _ => return Err(io::Error::last_os_error()),
        }
        if let Some((owner, deadline)) = gate.owner() {
            if clock() > deadline {
                terminate(owner)?;
            }
        }
        if clock() > request.deadline.saturating_add(5000) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Recovery lock did not settle",
            ));
        }
    }
}
fn write(path: &Path, value: &impl serde::Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    match super::write_new(path, &bytes) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn supervise(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let admission = super::read_admission(path)?;
    let request = &admission.request;
    let gate = Gate::open(request)?;
    let Some(_lock) = acquire(request, &gate)? else {
        return Ok(());
    };
    gate.set_owner(request.deadline + 35)?;
    if fault(path, "guard-crash-before-handoff") {
        std::process::exit(23);
    }
    if fault(path, "guard-hang-before-handoff") {
        std::thread::sleep(Duration::from_secs(3));
    }
    if super::fault_name().is_some_and(|fault| {
        fault.starts_with("native-") || fault.contains("replay") || fault.contains("after-spawn")
    }) {
        gate.cancel();
    }
    if gate.destination() == Destination::Pending && clock() < request.deadline {
        let worker_path = path.to_owned();
        let worker = std::thread::spawn(move || {
            let _ = worker(&worker_path);
        });
        while !worker.is_finished() && clock() < request.deadline + 10 {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    gate.reclaim_dead_ui();
    if gate.destination() == Destination::Pending {
        gate.cancel();
    }
    let publishing_deadline = clock() + 100;
    while gate.destination() == Destination::Publishing && clock() < publishing_deadline {
        gate.reclaim_dead_ui();
        std::thread::sleep(Duration::from_millis(2));
    }
    if gate.destination() == Destination::Navi {
        write(
            &path.with_extension("decision.json"),
            &serde_json::json!({"destination":"navi","id":request.id,"tick":clock(),"visible_tick":gate.visible_tick(),"hwnd":gate.window(),"ui":gate.ui()}),
        )?;
        return Ok(());
    }
    if gate.destination() == Destination::Publishing {
        return Err("Presentation worker did not settle".into());
    }
    if gate.destination() == Destination::NativeDone {
        return Ok(());
    }
    if fault(path, "guard-crash-before-replay") {
        std::process::exit(23);
    }
    native_replay(path, request, &gate)?;
    Ok(())
}
pub(super) fn worker(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let admission = super::read_admission(path)?;
    if fault(path, "worker-crash-before-handoff") {
        std::process::exit(23);
    }
    if fault(path, "worker-hang-before-handoff") {
        std::thread::sleep(Duration::from_secs(3));
    }
    let result = super::deliver_inner(admission.request, Some(&path.with_extension("commit")));
    write(&path.with_extension("worker.json"), &result)?;
    Ok(())
}
fn native_replay(
    path: &Path,
    request: &Request,
    gate: &Gate,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut attempts = 0;
    loop {
        gate.set_owner(clock() + 5500)?;
        if let Some(stamp) = gate.native_process() {
            if transport::alive(stamp).is_none() {
                return Err("Native dispatcher liveness is unknown; replay suppressed".into());
            }
            if transport::alive(stamp) == Some(true) {
                if fault(path, "guard-crash-during-replay") {
                    std::process::exit(23);
                }
                let mut deadline = if gate.native_started() == 0 {
                    clock() + 400
                } else {
                    gate.native_started().saturating_add(700)
                };
                while transport::alive(stamp) == Some(true)
                    && gate.destination() != Destination::NativeDone
                    && clock() < deadline
                {
                    if !native_interruptible(
                        gate.native_started(),
                        native_barrier(path, gate.native_started()),
                    ) {
                        deadline = gate.native_started().saturating_add(5000);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                if gate.destination() == Destination::NativeDone {
                    return Ok(());
                }
                if transport::alive(stamp) == Some(true) {
                    if !native_interruptible(
                        gate.native_started(),
                        native_barrier(path, gate.native_started()),
                    ) {
                        return Err("Native dispatch is still in flight; replay suppressed".into());
                    }
                    terminate(stamp)?;
                }
            }
            if transport::alive(stamp).is_none() {
                return Err("Native dispatcher liveness is unknown; replay suppressed".into());
            }
        }
        if reconcile_native(path, request, gate)? {
            return Ok(());
        }
        if attempts >= 2 {
            if gate.native_started() == 0 {
                let Some(_lock) = native_lock(request, gate)? else {
                    return Ok(());
                };
                gate.set_native_process(
                    transport::process_stamp(std::process::id()).map_err(error)?,
                );
                gate.calling_native();
                dispatch_native(path, request, gate)?;
                gate.native_accepted();
                write(&path.with_extension("native-direct.json"), &clock())?;
                return Ok(());
            }
            return Err("Native replay did not produce an observable folder".into());
        }
        attempts += 1;
        let mut native = command("--native", path)?.spawn()?;
        if fault(path, "guard-crash-after-spawn") {
            std::process::exit(23);
        }
        if fault(path, "guard-crash-during-replay") {
            std::process::exit(23);
        }
        let started = clock();
        let deadline = started + 900;
        while gate.destination() != Destination::NativeDone && clock() < deadline {
            if native.try_wait()?.is_some() {
                break;
            }
            if gate.native_started() == 0 && clock() > started + 400 {
                native.kill()?;
                native.wait()?;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if gate.destination() == Destination::NativeDone {
            return Ok(());
        }
    }
}
fn native_barrier(path: &Path, attempt: u64) -> io::Result<bool> {
    path.with_extension(format!("native-entered-{attempt}.json"))
        .try_exists()
}
fn native_interruptible(attempt: u64, entered: io::Result<bool>) -> bool {
    attempt == 0 || matches!(entered, Ok(false))
}
fn reconcile_native(
    path: &Path,
    request: &Request,
    gate: &Gate,
) -> Result<bool, Box<dyn std::error::Error>> {
    if gate.destination() == Destination::NativeDone {
        return Ok(true);
    }
    let attempt = gate.native_started();
    if attempt == 0 || gate.native_reconciled() == attempt {
        return Ok(false);
    }
    if !needs_reconciliation(
        super::read_admission(path)?.native_barrier,
        gate.native_process()
            .is_some_and(|stamp| transport::alive(stamp) == Some(false)),
        native_barrier(path, attempt),
    ) {
        gate.mark_native_reconciled(attempt);
        return Ok(false);
    }
    let before: Vec<usize> = fs::read(path.with_extension("native-before.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let deadline = attempt.saturating_add(700);
    loop {
        if let Some(window) = native_windows(request)?
            .into_iter()
            .find(|window| !before.contains(window))
        {
            gate.native_done(window);
            write(&path.with_extension("native-reconciled.json"), &window)?;
            return Ok(true);
        }
        if clock() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    gate.mark_native_reconciled(attempt);
    Ok(false)
}
fn native_lock(request: &Request, gate: &Gate) -> io::Result<Option<Lock>> {
    let identity = transport::identity(std::process::id()).map_err(error)?;
    let name = crate::explorer_host::security::wide(&format!(
        r"Local\Navi.ExplorerHost.Native.{}.{}.{}",
        identity.sid, identity.session, request.id
    ));
    let descriptor = crate::explorer_host::security::Descriptor::new(&format!(
        "D:P(A;;GA;;;SY)(A;;GA;;;{})",
        identity.sid
    ))?;
    let mutex = unsafe {
        Owned(
            CreateMutexW(Some(&descriptor.attributes()), false, PCWSTR(name.as_ptr()))
                .map_err(error)?,
        )
    };
    let deadline = clock() + 3000;
    loop {
        if matches!(
            gate.destination(),
            Destination::NativeDone | Destination::Navi
        ) {
            return Ok(None);
        }
        match unsafe { WaitForSingleObject(mutex.0, 20) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => {
                let lock = Lock(mutex);
                return Ok((!matches!(
                    gate.destination(),
                    Destination::NativeDone | Destination::Navi
                ))
                .then_some(lock));
            }
            WAIT_TIMEOUT => {}
            _ => return Err(io::Error::last_os_error()),
        }
        if clock() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Native dispatcher still owns this request",
            ));
        }
    }
}
pub(super) fn native_worker(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let admission = super::read_admission(path)?;
    let request = &admission.request;
    let gate = Gate::open(request)?;
    let Some(_lock) = native_lock(request, &gate)? else {
        return Ok(());
    };
    if reconcile_native(path, request, &gate)? {
        return Ok(());
    }
    gate.set_native_process(transport::process_stamp(std::process::id()).map_err(error)?);
    let prepared = super::com::NativeRequest::prepare()?;
    while matches!(
        gate.destination(),
        Destination::Pending | Destination::Publishing
    ) {
        gate.reclaim_dead_ui();
        if clock() >= request.deadline {
            gate.cancel();
        }
        if clock() > request.deadline.saturating_add(100) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    if gate.destination() != Destination::Native {
        return Ok(());
    }
    if fault(path, "native-crash-before-call") {
        std::process::exit(23);
    }
    if fault(path, "native-hang-before-call") {
        std::thread::sleep(Duration::from_secs(3));
    }
    let before_path = path.with_extension("native-before.json");
    if !before_path.exists() {
        write(&before_path, &native_handles()?)?;
    }
    gate.calling_native();
    if fault(path, "native-crash-after-intent") {
        std::process::exit(23);
    }
    dispatch_prepared(path, request, &gate, &prepared)?;
    gate.native_accepted();
    write(&path.with_extension("native-accepted.json"), &clock())?;
    if fault(path, "native-crash-after-call") {
        std::process::exit(23);
    }
    Ok(())
}
fn needs_reconciliation(barrier: bool, process_dead: bool, marker: io::Result<bool>) -> bool {
    !barrier || !process_dead || marker.unwrap_or(true)
}
fn dispatch_native(
    path: &Path,
    request: &Request,
    gate: &Gate,
) -> Result<(), Box<dyn std::error::Error>> {
    let prepared = super::com::NativeRequest::prepare()?;
    dispatch_prepared(path, request, gate, &prepared)
}
fn dispatch_prepared(
    path: &Path,
    request: &Request,
    gate: &Gate,
    prepared: &super::com::NativeRequest,
) -> Result<(), Box<dyn std::error::Error>> {
    prepared.show(request, || {
        write(
            &path.with_extension(format!("native-entered-{}.json", gate.native_started())),
            &request.id,
        )?;
        if fault(path, "native-crash-after-dispatch-barrier") {
            std::process::exit(23);
        }
        Ok(())
    })
}
fn native_handles() -> windows::core::Result<Vec<usize>> {
    unsafe extern "system" fn collect(hwnd: HWND, state: LPARAM) -> windows::core::BOOL {
        let mut class = [0u16; 64];
        let length = GetClassNameW(hwnd, &mut class) as usize;
        if String::from_utf16_lossy(&class[..length]) == "CabinetWClass" {
            let handles = &mut *(state.0 as *mut Vec<usize>);
            handles.push(hwnd.0 as usize);
        }
        true.into()
    }
    let mut handles = Vec::new();
    unsafe {
        EnumWindows(
            Some(collect),
            LPARAM((&mut handles as *mut Vec<usize>) as isize),
        )?;
    }
    Ok(handles)
}
fn native_windows(request: &Request) -> windows::core::Result<Vec<usize>> {
    unsafe {
        let text = crate::explorer_host::security::wide(&request.path);
        let mut pidl = std::ptr::null_mut();
        SHParseDisplayName(PCWSTR(text.as_ptr()), None, &mut pidl, 0, None)?;
        let location = windows::Win32::System::Variant::InitVariantFromBuffer(
            pidl.cast(),
            ILGetSize(Some(pidl)),
        );
        CoTaskMemFree(Some(pidl.cast()));
        let location = location?;
        let shell: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
        for kind in [SWC_EXPLORER, SWC_BROWSER] {
            let mut handle = 0;
            let _ = shell.FindWindowSW(
                &location,
                &VARIANT::default(),
                kind,
                &mut handle,
                ShellWindowFindWindowOptions(0),
            );
            let hwnd = GetAncestor(HWND(handle as isize as _), GA_ROOT);
            let mut class = [0u16; 64];
            let count = GetClassNameW(hwnd, &mut class) as usize;
            if String::from_utf16_lossy(&class[..count]) != "CabinetWClass"
                || !IsWindowVisible(hwnd).as_bool()
            {
                continue;
            }
            let browser = shell.FindWindowSW(
                &location,
                &VARIANT::default(),
                kind,
                &mut handle,
                SWFO_NEEDDISPATCH,
            )?;
            let browser: IWebBrowser2 = browser.cast()?;
            let view: IShellFolderViewDual = browser.Document()?.cast()?;
            let path = view
                .Folder()?
                .cast::<Folder2>()?
                .Self_()?
                .Path()?
                .to_string();
            if !path.eq_ignore_ascii_case(&request.path) {
                continue;
            }
            let selected = view.SelectedItems()?;
            let mut selection = Vec::new();
            for index in 0..selected.Count()? {
                selection.push(selected.Item(&VARIANT::from(index))?.Path()?.to_string());
            }
            if !request.selection.iter().all(|path| {
                selection
                    .iter()
                    .any(|selected| selected.eq_ignore_ascii_case(path))
            }) {
                continue;
            }
            return Ok(vec![hwnd.0 as usize]);
        }
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_dispatcher_is_not_interrupted_after_the_native_call_barrier() {
        assert!(native_interruptible(0, Ok(false)));
        assert!(native_interruptible(12, Ok(false)));
        assert!(!native_interruptible(12, Ok(true)));
        assert!(!native_interruptible(
            12,
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        ));
    }

    #[test]
    fn only_proven_death_before_a_required_native_barrier_skips_reconciliation() {
        assert!(!needs_reconciliation(true, true, Ok(false)));
        assert!(needs_reconciliation(false, true, Ok(false)));
        assert!(needs_reconciliation(true, false, Ok(false)));
        assert!(needs_reconciliation(true, true, Ok(true)));
        assert!(needs_reconciliation(
            true,
            true,
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        ));
    }
}
