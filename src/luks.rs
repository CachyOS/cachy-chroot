use crate::{block_device, utils};

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use subprocess::Exec;

pub fn open_device(device: &block_device::BlockDevice) -> bool {
    log::info!("Opening LUKS encrypted partition {}", device.name);
    let result = Exec::cmd("cryptsetup")
        .args(&["luksOpen", &device.name, &format!("luks-{}", &device.uuid)])
        .join();
    if result.is_err() || !result.unwrap().success() {
        utils::print_error_and_exit(&format!(
            "Failed to open LUKS encrypted partition {}",
            device.name
        ));
    }
    true
}

pub fn close_device(device: &block_device::BlockDevice) -> bool {
    log::info!("Closing LUKS encrypted partition {}", device.name);
    let result =
        Exec::cmd("cryptsetup").args(&["luksClose", &format!("luks-{}", &device.uuid)]).join();
    if result.is_err() || !result.unwrap().success() {
        log::warn!("Failed to close LUKS encrypted partition {}", device.name);
    }
    true
}

pub fn list_crypttab_entries(
    crypttab_path: &PathBuf,
    has_luks_on_root: bool,
) -> HashMap<String, String> {
    if !crypttab_path.exists() {
        if has_luks_on_root {
            log::warn!(
                "Unable to find /etc/crypttab in the root partition, is this a valid root \
                 partition? Good luck fixing that!"
            );
        }
        return HashMap::new();
    }

    let contents = fs::read_to_string(crypttab_path);
    if contents.is_err() {
        log::error!("Failed to read /etc/crypttab, skipping...");
        return HashMap::new();
    }
    let mut crypttab_entries: HashMap<String, String> = HashMap::new();

    let contents = contents.unwrap();
    for line in contents.lines() {
        if line.starts_with('#') {
            continue;
        }
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if parts.len() < 2 {
            log::warn!("Invalid crypttab entry, skipping...");
            continue;
        }
        let device = parts[1].trim_start_matches("UUID=");
        crypttab_entries.insert(parts[0].into(), device.into());
    }

    crypttab_entries
}
