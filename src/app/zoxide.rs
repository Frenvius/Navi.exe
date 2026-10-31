use std::{
    io::{self, Read},
    os::windows::{io::AsRawHandle, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::HANDLE,
    System::{
        Pipes::PeekNamedPipe,
        Threading::{WaitForSingleObject, CREATE_NO_WINDOW},
    },
};
use winreg::{enums::HKEY_CURRENT_USER, RegKey};

const ROOT: &str = r"Software\Navi";
const OUTPUT_LIMIT: usize = 32 * 1024;
const TIMEOUT: Duration = Duration::from_millis(400);
const SUGGESTION_LIMIT: usize = 5;

fn executable_cache() -> &'static std::sync::Mutex<Option<PathBuf>> {
    static EXECUTABLE: OnceLock<std::sync::Mutex<Option<PathBuf>>> = OnceLock::new();
    EXECUTABLE.get_or_init(|| {
        let path = if super::zoxide_install::simulation().is_some() {
            None
        } else {
            find_in_path("zoxide.exe")
        };
        std::sync::Mutex::new(path)
    })
}

pub fn executable() -> Option<PathBuf> {
    executable_cache().lock().ok()?.clone()
}

pub(super) fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .filter(|path| path.is_absolute())
            .map(|path| path.join(name))
            .find(|path| path.is_file())
    })
}

fn expand_registered(value: &str, variables: &std::collections::HashMap<String, String>) -> String {
    let mut value = value.to_string();
    for _ in 0..8 {
        let mut out = String::new();
        let mut rest = value.as_str();
        while let Some(start) = rest.find('%') {
            let Some(end) = rest[start + 1..].find('%') else {
                break;
            };
            let end = start + 1 + end;
            out.push_str(&rest[..start]);
            let name = &rest[start + 1..end];
            if let Some(replacement) = variables.get(&name.to_uppercase()) {
                out.push_str(replacement);
            } else if let Ok(replacement) = std::env::var(name) {
                out.push_str(&replacement);
            } else {
                out.push_str(&rest[start..=end]);
            }
            rest = &rest[end + 1..];
            if out.len() > 32768 {
                return value;
            }
        }
        out.push_str(rest);
        if out == value || out.len() > 32768 {
            break;
        }
        value = out;
    }
    value
}

fn registered_paths() -> Vec<PathBuf> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let mut variables = std::collections::HashMap::new();
    let mut paths = Vec::new();
    for (root, path) in [
        (
            HKEY_LOCAL_MACHINE,
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
        ),
        (HKEY_CURRENT_USER, r"Environment"),
    ] {
        if let Ok(key) = RegKey::predef(root).open_subkey(path) {
            if let Ok(path) = key.get_value::<String, _>("Path") {
                paths.push(path);
            }
            for (name, _) in key.enum_values().flatten().take(256) {
                if !name.eq_ignore_ascii_case("PATH") {
                    if let Ok(value) = key.get_value::<String, _>(&name) {
                        variables.insert(name.to_uppercase(), value);
                    }
                }
            }
        }
    }
    paths
        .into_iter()
        .flat_map(|path| {
            std::env::split_paths(&expand_registered(&path, &variables)).collect::<Vec<_>>()
        })
        .collect()
}

fn discover(paths: impl IntoIterator<Item = PathBuf>, roots: &[PathBuf]) -> Option<PathBuf> {
    for path in paths {
        let executable = path.join("zoxide.exe");
        if path.is_absolute() && executable.is_file() {
            return Some(executable);
        }
    }
    for root in roots {
        let link = root.join("Links").join("zoxide.exe");
        if link.is_file() {
            return Some(link);
        }
        if let Ok(packages) = std::fs::read_dir(root.join("Packages")) {
            for package in packages.flatten().take(2048) {
                if package
                    .file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .starts_with("ajeetdsouza.zoxide_")
                {
                    let path = package.path().join("zoxide.exe");
                    if path.is_file() {
                        return Some(path);
                    }
                    if let Ok(children) = std::fs::read_dir(package.path()) {
                        for child in children.flatten().take(32) {
                            let path = child.path().join("zoxide.exe");
                            if path.is_file() {
                                return Some(path);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

pub(super) fn rediscover() -> Option<PathBuf> {
    let mut paths = registered_paths();
    if let Some(process_path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&process_path));
    }
    let roots: Vec<_> = [
        std::env::var_os("LOCALAPPDATA")
            .map(|path| PathBuf::from(path).join("Microsoft").join("WinGet")),
        std::env::var_os("PROGRAMFILES").map(|path| PathBuf::from(path).join("WinGet")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let found = discover(paths, &roots);
    if let Ok(mut cached) = executable_cache().lock() {
        *cached = found.clone();
    }
    found
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Integration,
    Record,
}

impl Setting {
    fn value(self) -> &'static str {
        match self {
            Self::Integration => "ZoxideEnabled",
            Self::Record => "ZoxideRecordNavigations",
        }
    }

    pub fn save(self, enabled: bool) -> io::Result<()> {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(ROOT)?;
        key.set_value(self.value(), &u32::from(enabled))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Preferences {
    pub available: bool,
    pub enabled: bool,
    pub record: bool,
}

impl Preferences {
    pub fn read() -> Self {
        let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey(ROOT).ok();
        Self::read_from(key.as_ref(), executable().is_some())
    }

    fn read_from(key: Option<&RegKey>, available: bool) -> Self {
        let enabled = |setting: Setting| {
            available
                && key
                    .and_then(|key| key.get_value::<u32, _>(setting.value()).ok())
                    .map_or(true, |value| value == 1)
        };
        Self {
            available,
            enabled: enabled(Setting::Integration),
            record: enabled(Setting::Record),
        }
    }
}

pub fn eligible(text: &str) -> bool {
    let text = text.trim().trim_matches('"');
    !text.is_empty()
        && text.len() <= 2048
        && !text.contains(['\\', '/', ':', '\0', '\r', '\n'])
        && !text.starts_with(['.', '~', '%', '$'])
}

fn run_command(command: &mut Command, timeout: Duration) -> Option<String> {
    let mut child = command
        .creation_flags(CREATE_NO_WINDOW.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let result = (|| {
        let start = Instant::now();
        let mut output = Vec::new();
        let mut stdout = child.stdout.take()?;
        loop {
            let status = child.try_wait().ok()?;
            let mut available = 0;
            let _ = unsafe {
                PeekNamedPipe(
                    HANDLE(stdout.as_raw_handle()),
                    None,
                    0,
                    None,
                    Some(&mut available),
                    None,
                )
            };
            if available != 0 {
                if output.len() + available as usize > OUTPUT_LIMIT {
                    return None;
                }
                let mut buffer = [0; 4096];
                let size = buffer.len().min(available as usize);
                let read = stdout.read(&mut buffer[..size]).ok()?;
                if read == 0 {
                    return None;
                }
                output.extend_from_slice(&buffer[..read]);
            } else if let Some(status) = status {
                return status
                    .success()
                    .then(|| String::from_utf8(output).ok())
                    .flatten();
            }
            if start.elapsed() >= timeout {
                return None;
            }
            if available == 0 {
                unsafe {
                    WaitForSingleObject(HANDLE(child.as_raw_handle()), 5);
                }
            }
        }
    })();
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

fn query(text: &str, list: bool) -> Option<String> {
    if !eligible(text) || !Preferences::read().enabled {
        return None;
    }
    let mut command = Command::new(executable()?);
    command.arg("query");
    if list {
        command.args(["--list", "--score"]);
    }
    command
        .arg("--")
        .args(text.trim().trim_matches('"').split_whitespace());
    run_command(&mut command, TIMEOUT)
}

fn path_key(path: &str) -> String {
    path.trim_matches('"')
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

fn append_matches(items: &mut Vec<String>, output: &str) {
    let mut added = 0;
    for line in output.lines() {
        let line = line.trim_start();
        let Some((score, path)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let path = path.trim_start();
        if score.parse::<f64>().is_err() || !Path::new(path).is_absolute() {
            continue;
        }
        let key = path_key(path);
        if items.iter().any(|item| path_key(item) == key) || !Path::new(path).is_dir() {
            continue;
        }
        items.push(path.to_string());
        added += 1;
        if added == SUGGESTION_LIMIT {
            break;
        }
    }
}

pub fn suggestions(text: &str, items: &mut Vec<String>) {
    if let Some(output) = query(text, true) {
        append_matches(items, &output);
    }
}

pub fn jump(text: &str) -> Option<String> {
    let output = query(text, false)?;
    let path = output.trim_end_matches(['\r', '\n']);
    (Path::new(path).is_absolute() && !path.contains(['\r', '\n']) && Path::new(path).is_dir())
        .then(|| path.to_string())
}

pub fn record(path: &str) {
    if !Preferences::read().record || !Path::new(path).is_absolute() || !Path::new(path).is_dir() {
        return;
    }
    if let Some(executable) = executable() {
        let _ = run_command(
            Command::new(executable).arg("add").arg("--").arg(path),
            TIMEOUT,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_leave_paths_protocols_and_environment_to_the_shell() {
        for text in [
            "",
            r"C:\Windows",
            r"\\server\share",
            "https://example.com",
            "shell:Downloads",
            "::CLSID",
            "../foo",
            "%TEMP%",
            "$HOME",
            "~",
            "a\nb",
        ] {
            assert!(!eligible(text), "{text}");
        }
        for text in ["navi", "workspace projects", "área de trabalho", "--help"] {
            assert!(eligible(text), "{text}");
        }
    }

    #[test]
    fn rediscovery_uses_fresh_paths_links_and_portable_package_directories() {
        let base =
            std::env::temp_dir().join(format!("navi-zoxide-discovery-{}", std::process::id()));
        let fresh = base.join("new PATH");
        let links = base.join("WinGet").join("Links");
        let package = base
            .join("WinGet")
            .join("Packages")
            .join("ajeetdsouza.zoxide_source");
        let nested = package.join("zoxide-x86_64");
        let roots = [base.join("WinGet")];
        for dir in [&fresh, &links, &nested] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let locations: Vec<_> = [&fresh, &links, &package, &nested]
            .into_iter()
            .map(|dir| dir.join("zoxide.exe"))
            .collect();
        for file in &locations {
            std::fs::write(file, b"fixture").unwrap();
        }
        assert_eq!(
            discover([fresh.clone()], &roots),
            Some(locations[0].clone())
        );
        std::fs::remove_file(&locations[0]).unwrap();
        assert_eq!(
            discover([fresh.clone()], &roots),
            Some(locations[1].clone())
        );
        std::fs::remove_file(&locations[1]).unwrap();
        assert_eq!(discover([], &roots), Some(locations[2].clone()));
        std::fs::remove_file(&locations[2]).unwrap();
        assert_eq!(discover([], &roots), Some(locations[3].clone()));
        std::fs::remove_file(&locations[3]).unwrap();
        assert_eq!(discover([], &roots), None);
        let variables = std::collections::HashMap::from([
            ("NEW_HOME".into(), fresh.to_string_lossy().into_owned()),
            ("NESTED_HOME".into(), "%NEW_HOME%".into()),
        ]);
        assert_eq!(
            expand_registered("%nested_home%", &variables),
            fresh.to_string_lossy()
        );
        assert_eq!(
            expand_registered("%MISSING_NAVI_TEST%", &variables),
            "%MISSING_NAVI_TEST%"
        );
        for dir in [
            &nested,
            &package,
            &package.parent().unwrap().to_path_buf(),
            &links,
            &roots[0],
            &fresh,
            &base,
        ] {
            std::fs::remove_dir(dir).unwrap();
        }
    }

    #[test]
    fn preferences_are_independent_and_unavailable_always_disables_them() {
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let path = format!(r"Software\NaviZoxideTest-{}", std::process::id());
        let (key, _) = root.create_subkey(&path).unwrap();
        assert_eq!(Preferences::read_from(None, false), Preferences::default());
        assert_eq!(
            Preferences::read_from(Some(&key), true),
            Preferences {
                available: true,
                enabled: true,
                record: true
            }
        );
        key.set_value(Setting::Integration.value(), &0u32).unwrap();
        assert_eq!(
            Preferences::read_from(Some(&key), true),
            Preferences {
                available: true,
                enabled: false,
                record: true
            }
        );
        key.set_value(Setting::Record.value(), &0u32).unwrap();
        assert!(!Preferences::read_from(Some(&key), true).record);
        key.set_value(Setting::Integration.value(), &1u32).unwrap();
        assert_eq!(
            Preferences::read_from(Some(&key), false),
            Preferences::default()
        );
        drop(key);
        root.delete_subkey(path).unwrap();
    }

    #[test]
    fn child_process() {
        match std::env::var("NAVI_ZOXIDE_CHILD").as_deref() {
            Ok("timeout") => std::thread::sleep(Duration::from_secs(3)),
            Ok("large") => print!("{}", "x".repeat(OUTPUT_LIMIT * 3)),
            Ok("failure") => std::process::exit(1),
            _ => print!("zoxide-test-output"),
        }
    }

    #[test]
    fn process_transport_bounds_output_timeout_and_failures() {
        for mode in ["success", "large", "timeout", "failure"] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "app::zoxide::tests::child_process",
                    "--nocapture",
                ])
                .env("NAVI_ZOXIDE_CHILD", mode);
            let start = Instant::now();
            let timeout = if mode == "timeout" {
                Duration::from_millis(150)
            } else {
                Duration::from_secs(1)
            };
            let output = run_command(&mut command, timeout);
            if mode == "success" {
                assert!(output.unwrap().contains("zoxide-test-output"));
            } else {
                assert!(output.is_none(), "{mode}");
            }
            assert!(start.elapsed() < Duration::from_secs(2), "{mode}");
        }
        assert!(run_command(&mut Command::new(r"C:\missing-navi-zoxide.exe"), TIMEOUT).is_none());
    }

    #[test]
    fn scored_matches_append_after_history_and_deduplicate_windows_spelling() {
        let base = std::env::temp_dir().join(format!("navi-zoxide-matches-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let mut output = String::new();
        let mut dirs = Vec::new();
        for index in 0..8 {
            let dir = base.join(format!("pasta com espaços {index}"));
            std::fs::create_dir(&dir).unwrap();
            output.push_str(&format!("  {} {}\r\n", 100 - index, dir.display()));
            dirs.push(dir);
        }
        let first = dirs[0].to_string_lossy().to_uppercase().replace('\\', "/") + "/";
        let mut items = vec!["history-command".into(), first.clone()];
        append_matches(&mut items, &output);
        assert_eq!(items.len(), 7);
        assert_eq!(items[0], "history-command");
        assert_eq!(items[1], first);
        assert_eq!(items[2], dirs[1].to_string_lossy());
        for dir in dirs {
            std::fs::remove_dir(dir).unwrap();
        }
        std::fs::remove_dir(base).unwrap();
    }
}
