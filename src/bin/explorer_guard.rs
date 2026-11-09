#![windows_subsystem = "windows"]

fn main() {
    navi::crash::install();
    if let Err(error) = navi::explorer_host::guardian::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
