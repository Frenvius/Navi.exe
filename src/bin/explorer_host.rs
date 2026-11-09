#![windows_subsystem = "windows"]

#[path = "../explorer_host/broker.rs"]
mod broker;
use navi::explorer_host;

fn main() {
    navi::crash::install();
    if let Err(error) = broker::run() {
        eprintln!("Explorer host: {error}");
        std::process::exit(1);
    }
}
