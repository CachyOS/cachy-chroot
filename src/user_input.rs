use crate::block_device;

use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};

fn confirm_user_action<'a>(prompt_text: &'a str, theme: &'a ColorfulTheme) -> Confirm<'a> {
    Confirm::with_theme(theme)
        .with_prompt(prompt_text)
        .default(false)
        .show_default(false)
        .wait_for_newline(true)
}

pub fn mount_additional_partitions() -> bool {
    confirm_user_action(
        "Do you want to mount additional partitions?",
        &ColorfulTheme::default(),
    )
    .interact()
    .unwrap()
}

pub fn continue_on_mount_failure() -> bool {
    confirm_user_action(
        "Do you want to skip mounting this partition?",
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
    subvolumes: &[block_device::BTRFSSubVolume],
) -> block_device::BTRFSSubVolume {
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

pub fn get_block_device(
    partition_name: &str,
    block_devices: &[block_device::BlockDevice],
    allow_skip: bool,
) -> Option<block_device::BlockDevice> {
    let default_theme = ColorfulTheme::default();
    let prompt = Select::with_theme(&default_theme)
        .with_prompt(format!(
            "Select the block device for the {} partition (use arrow keys): ",
            partition_name.yellow()
        ))
        .default(0)
        .max_length(10)
        .items(block_devices);
    let index = if allow_skip {
        prompt.item("Skip").interact().ok()?
    } else {
        prompt.interact().ok()?
    };
    if index == block_devices.len() {
        return None;
    }
    Some(block_devices[index].clone())
}
