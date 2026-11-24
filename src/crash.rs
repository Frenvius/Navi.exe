use std::{
    fs,
    os::windows::io::{AsRawHandle, IntoRawHandle},
    panic::{self, PanicHookInfo},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
};

use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{HANDLE, HMODULE},
        System::{
            Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE},
            Diagnostics::Debug::{
                MiniDumpWithIndirectlyReferencedMemory, MiniDumpWithThreadInfo,
                MiniDumpWithUnloadedModules, MiniDumpWriteDump, SetUnhandledExceptionFilter,
                EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MINIDUMP_TYPE,
            },
            LibraryLoader::{
                GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
                GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            },
            SystemInformation::GetLocalTime,
            Threading::{
                GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
                SetThreadStackGuarantee,
            },
        },
    },
};

const KEPT_FILES: usize = 40;
const KEPT_LOGS: usize = 20;
const STACK_GUARANTEE: u32 = 64 * 1024;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

struct Reporter {
    dir: PathBuf,
    process: String,
    executable: String,
}

static REPORTER: OnceLock<Reporter> = OnceLock::new();
static NATIVE_REPORTING: AtomicBool = AtomicBool::new(false);

pub fn install() {
    let Some(root) = std::env::var_os("LOCALAPPDATA").map(|root| PathBuf::from(root).join("Navi"))
    else {
        return;
    };
    let executable = std::env::current_exe().unwrap_or_default();
    let process = executable
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "navi".to_owned());
    redirect_stderr(&root.join("logs"), &process);
    let dir = root.join("crashes");
    if let Err(error) = fs::create_dir_all(&dir) {
        eprintln!("crash reports disabled: {error}");
        return;
    }
    prune(&dir, KEPT_FILES);
    let reporter = Reporter {
        dir,
        process,
        executable: executable.display().to_string(),
    };
    if REPORTER.set(reporter).is_err() {
        return;
    }
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        write_panic_report(info);
        previous(info);
    }));
    unsafe {
        SetUnhandledExceptionFilter(Some(on_unhandled_exception));
        let mut guarantee = STACK_GUARANTEE;
        let _ = SetThreadStackGuarantee(&mut guarantee);
    }
}

fn redirect_stderr(dir: &Path, process: &str) {
    let attached = unsafe { GetStdHandle(STD_ERROR_HANDLE) }
        .is_ok_and(|handle| !handle.is_invalid());
    if attached || fs::create_dir_all(dir).is_err() {
        return;
    }
    prune(dir, KEPT_LOGS);
    let Ok(file) = fs::File::create(dir.join(format!("{}.log", process_stamp(process)))) else {
        return;
    };
    let handle = HANDLE(file.into_raw_handle());
    unsafe {
        let _ = SetStdHandle(STD_ERROR_HANDLE, handle);
    }
}

fn prune(dir: &Path, kept: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        if metadata.len() == 0 {
            let _ = fs::remove_file(entry.path());
            continue;
        }
        files.push((metadata.modified().ok(), entry.path()));
    }
    if files.len() <= kept {
        return;
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in files.into_iter().skip(kept) {
        let _ = fs::remove_file(path);
    }
}

fn process_stamp(process: &str) -> String {
    let time = unsafe { GetLocalTime() };
    format!(
        "{process}-{:04}{:02}{:02}-{:02}{:02}{:02}-{}",
        time.wYear,
        time.wMonth,
        time.wDay,
        time.wHour,
        time.wMinute,
        time.wSecond,
        unsafe { GetCurrentProcessId() }
    )
}

fn report_base(reporter: &Reporter) -> PathBuf {
    let tid = unsafe { GetCurrentThreadId() };
    reporter
        .dir
        .join(format!("{}-{tid}", process_stamp(&reporter.process)))
}

fn header(reporter: &Reporter, kind: &str) -> String {
    let time = unsafe { GetLocalTime() };
    format!(
        "Navi {}\nkind: {kind}\nexecutable: {}\npid: {}\ntime: {:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}\n",
        env!("CARGO_PKG_VERSION"),
        reporter.executable,
        unsafe { GetCurrentProcessId() },
        time.wYear,
        time.wMonth,
        time.wDay,
        time.wHour,
        time.wMinute,
        time.wSecond,
        time.wMilliseconds
    )
}

pub fn report_fatal(message: &str) {
    write_thread_report("fatal error", &format!("message: {message}\n"));
}

fn write_panic_report(info: &PanicHookInfo) {
    let location = info
        .location()
        .map(ToString::to_string)
        .unwrap_or_default();
    write_thread_report(
        "panic",
        &format!(
            "message: {}\nlocation: {location}\n",
            info.payload_as_str().unwrap_or("<non-string payload>")
        ),
    );
}

fn write_thread_report(kind: &str, details: &str) {
    let Some(reporter) = REPORTER.get() else {
        return;
    };
    let base = report_base(reporter);
    let thread = std::thread::current();
    let text = format!(
        "{}thread: {} ({})\n{details}\n{}\n",
        header(reporter, kind),
        thread.name().unwrap_or("<unnamed>"),
        unsafe { GetCurrentThreadId() },
        std::backtrace::Backtrace::force_capture()
    );
    let _ = fs::write(base.with_extension("txt"), text);
    write_dump_from_helper(base.with_extension("dmp"), None);
}

unsafe extern "system" fn on_unhandled_exception(pointers: *const EXCEPTION_POINTERS) -> i32 {
    if NATIVE_REPORTING.swap(true, Ordering::SeqCst) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    if let Some(reporter) = REPORTER.get() {
        let base = report_base(reporter);
        let text = format!(
            "{}thread: {}\n{}",
            header(reporter, "native exception"),
            GetCurrentThreadId(),
            describe_exception(pointers)
        );
        let _ = fs::write(base.with_extension("txt"), text);
        write_dump_from_helper(base.with_extension("dmp"), Some(pointers));
    }
    EXCEPTION_CONTINUE_SEARCH
}

unsafe fn describe_exception(pointers: *const EXCEPTION_POINTERS) -> String {
    let Some(record) = pointers.as_ref().and_then(|p| p.ExceptionRecord.as_ref()) else {
        return "exception record unavailable\n".to_owned();
    };
    let address = record.ExceptionAddress as usize;
    let parameters = record
        .ExceptionInformation
        .iter()
        .take(record.NumberParameters as usize)
        .map(|value| format!("{value:#x}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "code: {:#010x}\naddress: {address:#x}\nmodule: {}\nparameters: [{parameters}]\n",
        record.ExceptionCode.0 as u32,
        module_at(address)
    )
}

unsafe fn module_at(address: usize) -> String {
    let mut module = HMODULE::default();
    if GetModuleHandleExW(
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        PCWSTR(address as *const u16),
        &mut module,
    )
    .is_err()
    {
        return "<unknown>".to_owned();
    }
    let mut name = [0u16; 260];
    let length = GetModuleFileNameW(Some(module), &mut name) as usize;
    format!(
        "{}+{:#x}",
        String::from_utf16_lossy(&name[..length]),
        address - module.0 as usize
    )
}

fn write_dump_from_helper(path: PathBuf, pointers: Option<*const EXCEPTION_POINTERS>) {
    let thread_id = unsafe { GetCurrentThreadId() };
    let pointers = pointers.map(|pointers| pointers as usize);
    if let Ok(helper) = std::thread::Builder::new()
        .name("crash-dump".to_owned())
        .spawn(move || write_dump(&path, thread_id, pointers))
    {
        let _ = helper.join();
    }
}

fn write_dump(path: &Path, thread_id: u32, pointers: Option<usize>) {
    let Ok(file) = fs::File::create(path) else {
        return;
    };
    let exception = pointers.map(|pointers| MINIDUMP_EXCEPTION_INFORMATION {
        ThreadId: thread_id,
        ExceptionPointers: pointers as *mut EXCEPTION_POINTERS,
        ClientPointers: false.into(),
    });
    let kind = MINIDUMP_TYPE(
        MiniDumpWithThreadInfo.0
            | MiniDumpWithUnloadedModules.0
            | MiniDumpWithIndirectlyReferencedMemory.0,
    );
    let written = unsafe {
        MiniDumpWriteDump(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            HANDLE(file.as_raw_handle()),
            kind,
            exception.as_ref().map(|exception| exception as *const _),
            None,
            None,
        )
    };
    if written.is_err() {
        drop(file);
        let _ = fs::remove_file(path);
    }
}
