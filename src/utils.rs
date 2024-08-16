use std::process::exit;

pub fn print_error_and_exit(msg: &str) {
    log::error!("{msg}");
    exit(1);
}
