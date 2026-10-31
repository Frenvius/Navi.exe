use std::{
    io::Read,
    os::windows::{io::AsRawHandle, process::CommandExt},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::HANDLE,
    System::{
        Pipes::PeekNamedPipe,
        Threading::{WaitForSingleObject, CREATE_NO_WINDOW},
    },
};

pub const INSTALL_URL: &str = "https://github.com/ajeetdsouza/zoxide#installation";
const INSTALL_ARGS: [&str; 7] = [
    "install",
    "--id",
    "ajeetdsouza.zoxide",
    "-e",
    "--silent",
    "--accept-package-agreements",
    "--accept-source-agreements",
];
const OUTPUT_LIMIT: usize = 8192;
const TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Launch(String),
    Exit(u32, String),
    Timeout,
    NotFound,
    Settings(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Idle,
    Installing,
    Installed,
    Failed(Failure),
}

fn shared_state() -> &'static Mutex<State> {
    static STATE: Mutex<State> = Mutex::new(State::Idle);
    &STATE
}

pub fn state() -> State {
    shared_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

pub(super) fn simulation() -> Option<String> {
    #[cfg(debug_assertions)]
    {
        std::env::var("NAVI_ZOXIDE_INSTALL_TEST")
            .ok()
            .filter(|value| {
                matches!(
                    value.as_str(),
                    "success" | "failure" | "no-winget" | "not-found" | "timeout"
                )
            })
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

fn winget() -> Option<PathBuf> {
    if simulation().as_deref() == Some("no-winget") {
        return None;
    }
    static WINGET: OnceLock<Option<PathBuf>> = OnceLock::new();
    WINGET
        .get_or_init(|| {
            super::zoxide::find_in_path("winget.exe").or_else(|| {
                let path = PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
                    .join("Microsoft")
                    .join("WindowsApps")
                    .join("winget.exe");
                path.is_file().then_some(path)
            })
        })
        .clone()
}

pub fn winget_available() -> bool {
    winget().is_some()
}

fn drain(pipe: &mut (impl Read + AsRawHandle), output: &mut Vec<u8>) -> usize {
    let mut available = 0;
    let _ = unsafe {
        PeekNamedPipe(
            HANDLE(pipe.as_raw_handle()),
            None,
            0,
            None,
            Some(&mut available),
            None,
        )
    };
    if available == 0 {
        return 0;
    }
    let mut bytes = [0; 4096];
    let capacity = bytes.len().min(available as usize);
    let size = pipe.read(&mut bytes[..capacity]).unwrap_or(0);
    output.extend_from_slice(&bytes[..size]);
    if output.len() > OUTPUT_LIMIT {
        output.drain(..output.len() - OUTPUT_LIMIT);
    }
    size
}

fn detail(output: &[u8]) -> String {
    let clean = String::from_utf8_lossy(output)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>();
    clean
        .chars()
        .skip(clean.chars().count().saturating_sub(600))
        .collect()
}

fn run(command: &mut Command, timeout: Duration) -> Result<(), Failure> {
    let mut child = command
        .creation_flags(CREATE_NO_WINDOW.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Failure::Launch(error.to_string()))?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut output = Vec::new();
    let start = Instant::now();
    let result = loop {
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => break Err(Failure::Launch(error.to_string())),
        };
        let read = drain(&mut stdout, &mut output) + drain(&mut stderr, &mut output);
        if read == 0 {
            if let Some(status) = status {
                break if status.success() {
                    Ok(())
                } else {
                    Err(Failure::Exit(
                        status.code().unwrap_or(-1) as u32,
                        detail(&output),
                    ))
                };
            }
        }
        if start.elapsed() >= timeout {
            break Err(Failure::Timeout);
        }
        if read == 0 {
            unsafe {
                WaitForSingleObject(HANDLE(child.as_raw_handle()), 50);
            }
        }
    };
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

fn install() -> Result<(), Failure> {
    if let Some(mode) = simulation() {
        std::thread::sleep(Duration::from_millis(1800));
        match mode.as_str() {
            "success" => {}
            "timeout" => return Err(Failure::Timeout),
            "not-found" => return Err(Failure::NotFound),
            _ => return Err(Failure::Exit(42, "Simulated installer failure".into())),
        }
    } else if super::zoxide::rediscover().is_none() {
        let executable = winget().ok_or_else(|| Failure::Launch("winget.exe".into()))?;
        run(Command::new(executable).args(INSTALL_ARGS), TIMEOUT)?;
    }
    super::zoxide::rediscover().ok_or(Failure::NotFound)?;
    enable_discovered()
}

fn enable_discovered() -> Result<(), Failure> {
    let (key, _) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .create_subkey(r"Software\Navi")
        .map_err(|error| Failure::Settings(error.to_string()))?;
    enable(&key).map_err(|error| Failure::Settings(error.to_string()))
}

pub fn detect(enable_settings: bool) {
    if simulation().is_some() || state() == State::Installing {
        return;
    }
    if super::zoxide::rediscover().is_some() && enable_settings {
        *shared_state()
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = match enable_discovered() {
            Ok(()) => State::Installed,
            Err(error) => State::Failed(error),
        };
    }
}

fn enable(key: &winreg::RegKey) -> std::io::Result<()> {
    key.set_value("ZoxideEnabled", &1u32)?;
    key.set_value("ZoxideRecordNavigations", &1u32)
}

pub fn start(notify: impl Fn() + Send + 'static) -> bool {
    {
        let mut state = shared_state()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *state == State::Installing
            || super::zoxide::executable().is_some()
            || !winget_available()
        {
            return false;
        }
        *state = State::Installing;
    }
    match std::thread::Builder::new()
        .name("zoxide-install".into())
        .spawn(move || {
            notify();
            let result = install();
            *shared_state()
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = match result {
                Ok(()) => State::Installed,
                Err(error) => State::Failed(error),
            };
            notify();
        }) {
        Ok(_) => true,
        Err(error) => {
            *shared_state()
                .lock()
                .unwrap_or_else(|error| error.into_inner()) =
                State::Failed(Failure::Launch(error.to_string()));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_process() {
        match std::env::var("NAVI_INSTALL_CHILD").as_deref() {
            Ok("timeout") => std::thread::sleep(Duration::from_secs(3)),
            Ok("failure") => {
                eprintln!("network unavailable");
                std::process::exit(42);
            }
            Ok("large") => {
                for _ in 0..100 {
                    println!("{}", "x".repeat(4096));
                }
            }
            _ => {}
        }
    }

    #[test]
    fn installer_handles_exit_details_timeout_and_output_overflow() {
        assert_eq!(
            INSTALL_ARGS,
            [
                "install",
                "--id",
                "ajeetdsouza.zoxide",
                "-e",
                "--silent",
                "--accept-package-agreements",
                "--accept-source-agreements"
            ]
        );
        for mode in ["success", "failure", "large", "timeout"] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "app::zoxide_install::tests::child_process",
                    "--nocapture",
                ])
                .env("NAVI_INSTALL_CHILD", mode);
            let timeout = if mode == "timeout" {
                Duration::from_millis(150)
            } else {
                Duration::from_secs(2)
            };
            let start = Instant::now();
            let result = run(&mut command, timeout);
            match mode {
                "failure" => assert!(
                    matches!(result, Err(Failure::Exit(42, ref message)) if message.contains("network unavailable"))
                ),
                "timeout" => assert_eq!(result, Err(Failure::Timeout)),
                _ => assert_eq!(result, Ok(())),
            }
            assert!(start.elapsed() < Duration::from_secs(3));
        }
        assert!(matches!(
            run(&mut Command::new(r"C:\missing-winget-navi.exe"), TIMEOUT),
            Err(Failure::Launch(_))
        ));
        assert!(detail(&vec![b'x'; OUTPUT_LIMIT]).len() <= 600);
    }

    #[test]
    fn successful_install_enables_both_existing_disabled_settings() {
        let root = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
        let path = format!(r"Software\NaviZoxideInstallTest-{}", std::process::id());
        let (key, _) = root.create_subkey(&path).unwrap();
        key.set_value("ZoxideEnabled", &0u32).unwrap();
        key.set_value("ZoxideRecordNavigations", &0u32).unwrap();
        key.set_value("Unrelated", &123u32).unwrap();
        enable(&key).unwrap();
        assert_eq!(key.get_value::<u32, _>("ZoxideEnabled").unwrap(), 1);
        assert_eq!(
            key.get_value::<u32, _>("ZoxideRecordNavigations").unwrap(),
            1
        );
        assert_eq!(key.get_value::<u32, _>("Unrelated").unwrap(), 123);
        drop(key);
        root.delete_subkey(path).unwrap();
    }
}
