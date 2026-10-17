// Release builds are GUI-subsystem executables (no console window). Do not remove.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(error) = lmu_race_engineering_lib::run() {
        eprintln!("LMU Race Engineering failed to start: {error}");
        std::process::exit(1);
    }
}
