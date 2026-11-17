#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! DirectLook: Hızlı Bakış, Brysk'tan ayrı süreç.

fn main() {
    rf_app::look::app::main();
}
