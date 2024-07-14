use std::io;

use colored::Colorize;
use log::{Level, Metadata, Record};

struct SimpleLogger;

static LOGGER: SimpleLogger = SimpleLogger;

impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let level_str = match record.level() {
                Level::Error => "Error:".red(),
                Level::Warn => "Warning:".yellow(),
                Level::Info => "Info:".cyan(),
                Level::Debug => "Debug:".white(),
                Level::Trace => "Trace:".black(),
            };
            println!("{} {}", level_str, record.args());
        }
    }

    fn flush(&self) {
        use std::io::Write;
        io::stdout().flush().unwrap();
    }
}

pub fn init_logger() -> Result<(), log::SetLoggerError> {
    log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Info))
}
