use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;

pub(super) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("SW_MEMORY_PROBE").as_deref() == Ok("1"))
}

pub fn checkpoint(label: &str) {
    if !enabled() {
        return;
    }
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    counters.cb = std::mem::size_of_val(&counters) as u32;
    let result = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as _,
            counters.cb,
        )
    };
    if let Err(error) = result {
        eprintln!("native memory probe: {error}");
        return;
    }
    eprintln!(
        "native-memory|{}|{label}|{}|{}",
        start.elapsed().as_micros(),
        counters.WorkingSetSize,
        counters.PrivateUsage
    );
}

pub fn start_shell() {
    if enabled() {
        std::thread::sleep(Duration::from_millis(2000));
    }
    checkpoint("shell-start");
}

pub fn band_tick(before: f32, after: f32) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !ENABLED.get_or_init(|| std::env::var("SW_BAND_PROBE").as_deref() == Ok("1")) {
        return;
    }
    static START: OnceLock<Instant> = OnceLock::new();
    eprintln!(
        "native-band|{}|{}|{}|{}",
        START.get_or_init(Instant::now).elapsed().as_micros(),
        after - before,
        before,
        after
    );
}

pub fn work(label: &str, started: Instant) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var("SW_SCROLL_PROBE").as_deref() == Ok("1")) {
        eprintln!("native-work|{label}|{}", started.elapsed().as_micros());
    }
}

pub fn device_info(device: &windows::Win32::Graphics::Direct3D11::ID3D11Device) {
    if !enabled() {
        return;
    }
    use windows::core::Interface;
    let result = (|| -> windows::core::Result<()> {
        let dxgi: windows::Win32::Graphics::Dxgi::IDXGIDevice = device.cast()?;
        let desc = unsafe { dxgi.GetAdapter()?.GetDesc()? };
        eprintln!(
            "native-adapter|{}|flags={:#x}",
            String::from_utf16_lossy(&desc.Description).trim_end_matches('\0'),
            unsafe { device.GetCreationFlags() }
        );
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("native memory adapter: {error}");
    }
}
