use crate::zfs::{self, ZFSDataSetUtils};
use crate::{block_device, btrfs};

use colored::Colorize;
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, Input, MultiSelect, Select};

fn confirm_user_action<'a>(prompt_text: &'a str, theme: &'a ColorfulTheme) -> Confirm<'a> {
    Confirm::with_theme(theme)
        .with_prompt(prompt_text)
        .default(false)
        .show_default(false)
        .wait_for_newline(true)
}

pub fn mount_additional_partitions() -> bool {
    confirm_user_action("Do you want to mount additional partitions?", &ColorfulTheme::default())
        .interact()
        .unwrap()
}

pub fn continue_on_mount_failure() -> bool {
    confirm_user_action("Do you want to skip mounting this partition?", &ColorfulTheme::default())
        .interact()
        .unwrap()
}

pub fn allow_zfs_forced_import() -> bool {
    confirm_user_action(
        "Failed to import ZFS Pool, do you want to force zfs pool import?",
        &ColorfulTheme::default(),
    )
    .interact()
    .unwrap()
}

pub fn allow_zfs_forced_export() -> bool {
    confirm_user_action(
        "Failed to export ZFS Pool, do you want to force zfs pool export?",
        &ColorfulTheme::default(),
    )
    .interact()
    .unwrap()
}

pub fn retry_zfs_passphrase(dataset: &str) -> bool {
    confirm_user_action(
        &format!("Do you want to retry entering the ZFS passphrase for dataset: {}?", dataset),
        &ColorfulTheme::default(),
    )
    .interact()
    .unwrap()
}

pub fn use_cachyos_btrfs_preset() -> bool {
    confirm_user_action(
        "Do you want to use CachyOS BTRFS preset to auto mount root subvolume?",
        &ColorfulTheme::default(),
    )
    .interact()
    .unwrap()
}

pub fn get_mount_point() -> String {
    log::warn!(
        "NOTE: Mountpoint is ignored for ZFS datasets as they manage their own mountpoints. If \
         you want to select a ZFS dataset with a custom mountpoint, please do so manually after \
         chrooting. If you want to use default mountpoint for a ZFS dataset, just type '/' when \
         prompted for mountpoint."
    );
    Input::with_theme(&ColorfulTheme::default())
        .with_prompt(
            "Enter the mount point for additional partition (e.g. /boot) type 'skip' to cancel: ",
        )
        .validate_with(|input: &String| -> Result<(), &'static str> {
            if input.starts_with('/') || input.eq_ignore_ascii_case("skip") {
                Ok(())
            } else {
                Err("Mount point must start with /")
            }
        })
        .interact()
        .unwrap()
}

pub fn get_btrfs_subvolume(
    partition_name: &str,
    subvolumes: &[btrfs::BTRFSSubVolume],
) -> btrfs::BTRFSSubVolume {
    let index = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(format!(
            "Select the subvolume for the {} partition (use arrow keys): ",
            partition_name.yellow()
        ))
        .default(0)
        .max_length(10)
        .items(subvolumes)
        .interact()
        .unwrap();
    subvolumes[index].clone()
}

pub fn get_zfs_datasets(
    zfs_pool_name: &str,
    datasets: &[zfs::ZFSDataSet],
    allow_empty: bool,
) -> Vec<usize> {
    log::info!(
        "Use [Spacebar] to select datasets, and [Enter] to confirm selection. Some datasets may \
         already be mounted and will be indicated as such. Use arrow keys to navigate the list."
    );
    let default_theme = ColorfulTheme::default();
    let prompt = MultiSelect::with_theme(&default_theme)
        .with_prompt(format!(
            "Select the zfs datasets to import from the zfs pool: {}: ",
            zfs_pool_name.yellow()
        ))
        .max_length(10)
        .items_checked(
            datasets.iter().map(|dataset| (dataset, dataset.is_mounted())).collect::<Vec<_>>(),
        );
    let mut selection = prompt.clone().interact().unwrap();
    while selection.is_empty() && !allow_empty {
        log::error!("You must select at least one dataset, please try again.");
        selection = prompt.clone().interact().unwrap();
    }
    selection
}

pub fn get_block_device<'a>(
    partition_name: &str,
    block_devices: &'a [block_device::BlockDevice],
    allow_skip: bool,
) -> Option<&'a block_device::BlockDevice> {
    let default_theme = ColorfulTheme::default();
    let prompt = Select::with_theme(&default_theme)
        .with_prompt(format!(
            "Select the block device for the {} partition (use arrow keys): ",
            partition_name.yellow()
        ))
        .default(0)
        .max_length(10)
        .items(block_devices);
    let index =
        if allow_skip { prompt.item("Skip").interact().ok()? } else { prompt.interact().ok()? };
    if index == block_devices.len() {
        return None;
    }
    Some(&block_devices[index])
}
