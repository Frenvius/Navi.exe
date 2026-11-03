#![windows_subsystem = "windows"]

fn main() {
    if let Err(error) = navi::explorer_host::guardian::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
