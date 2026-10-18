// Only honoured in the binary crate root; in lib.rs it is silently ignored.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    autoowl_lib::run();
}
