use std::{
    fs,
    os::windows::io::AsRawHandle,
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
            Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId},
        },
    },
};

const KEPT_FILES: usize = 40;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

struct Reporter {
    dir: PathBuf,
    process: String,
    executable: String,
}

static REPORTER: OnceLock<Reporter> = OnceLock::new();
static NATIVE_REPORTING: AtomicBool = AtomicBool::new(false);

pub fn install() {
    let Some(dir) = std::env::var_os("LOCALAPPDATA")
        .map(|root| PathBuf::from(root).join("Navi").join("crashes"))
    else {
        return;
    };
    if let Err(error) = fs::create_dir_all(&dir) {
        eprintln!("crash reports disabled: {error}");
        return;
    }
    prune(&dir);
    let executable = std::env::current_exe().unwrap_or_default();
    let process = executable
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "navi".to_owned());
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
    }
}

fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata
                .is_file()
                .then(|| (metadata.modified().ok(), entry.path()))
        })
        .collect();
    if files.len() <= KEPT_FILES {
        return;
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in files.into_iter().skip(KEPT_FILES) {
        let _ = fs::remove_file(path);
    }
}

fn report_base(reporter: &Reporter) -> PathBuf {
    let time = unsafe { GetLocalTime() };
    let (pid, tid) = unsafe { (GetCurrentProcessId(), GetCurrentThreadId()) };
    reporter.dir.join(format!(
        "{}-{:04}{:02}{:02}-{:02}{:02}{:02}-{pid}-{tid}",
        reporter.process,
        time.wYear,
        time.wMonth,
        time.wDay,
        time.wHour,
        time.wMinute,
        time.wSecond
    ))
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

fn write_panic_report(info: &PanicHookInfo) {
    let Some(reporter) = REPORTER.get() else {
        return;
    };
    let base = report_base(reporter);
    let thread = std::thread::current();
    let location = info
        .location()
        .map(ToString::to_string)
        .unwrap_or_default();
    let text = format!(
        "{}thread: {} ({})\nmessage: {}\nlocation: {location}\n\n{}\n",
        header(reporter, "panic"),
        thread.name().unwrap_or("<unnamed>"),
        unsafe { GetCurrentThreadId() },
        info.payload_as_str().unwrap_or("<non-string payload>"),
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
