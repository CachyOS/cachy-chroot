pub mod args;
pub mod block_device;

use block_device::BlockOrSubvolumeID;
use clap::Parser;
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use nix::unistd::Uid;
use std::{collections::HashMap, path::Path, process::exit};
use subprocess::Exec;
use tempfile::TempDir;
use which::which;

fn print_info(msg: &str) {
    println!("{} {}", "Info:".cyan(), msg);
}

fn print_warning(msg: &str) {
    println!("{} {}", "Warning:".yellow(), msg);
}

fn print_error(msg: &str) {
    eprintln!("{} {}", "Error:".red(), msg);
}

fn print_error_and_exit(msg: &str) {
    print_error(msg);
    exit(1);
}

fn user_input_block_device(
    partition_name: &str,
    block_devices: Vec<block_device::BlockDevice>,
    allow_skip: bool,
) -> Option<block_device::BlockDevice> {
    let default_theme = ColorfulTheme::default();
    let prompt = Select::with_theme(&default_theme)
        .with_prompt(format!(
            "Select the block device for the {} partition (use arrow keys): ",
            partition_name.yellow()
        ))
        .default(0)
        .max_length(3)
        .items(&block_devices);
    let index = if allow_skip {
        prompt.item("Skip").interact().unwrap()
    } else {
        prompt.interact().unwrap()
    };
    if index == block_devices.len() {
        return None;
    }
    return Some(block_devices[index].clone());
}

fn user_input_btrfs_subvolume(
    partition_name: &str,
    subvolumes: Vec<block_device::BTRFSSubVolume>,
) -> block_device::BTRFSSubVolume {
    let index = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(format!(
            "Select the subvolume for the {} partition (use arrow keys): ",
            partition_name.yellow()
        ))
        .default(0)
        .max_length(3)
        .items(&subvolumes)
        .interact()
        .unwrap();
    return subvolumes[index].clone();
}

fn user_input_mount_additional_partitions() -> bool {
    Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt("Do you want to mount additional partitions?")
        .default(false)
        .show_default(false)
        .wait_for_newline(true)
        .interact()
        .unwrap()
}

fn user_input_continue_on_mount_failure() -> bool {
    Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt("Do you want to skip mounting this partition?")
        .default(false)
        .show_default(false)
        .wait_for_newline(true)
        .interact()
        .unwrap()
}

fn user_input_mount_point() -> String {
    Input::with_theme(&ColorfulTheme::default())
        .with_prompt(
            "Enter the mount point for additional partition (e.g. /boot) type 'skip' to cancel: ",
        )
        .validate_with(|input: &String| -> Result<(), &str> {
            if input.starts_with('/') || input.eq_ignore_ascii_case("skip") {
                Ok(())
            } else {
                Err("Mount point must start with /")
            }
        })
        .interact()
        .unwrap()
}

fn mount_block_device(
    device: &block_device::BlockDevice,
    mount_point: &str,
    gracefully_fail: bool,
    options: Option<Vec<String>>,
) {
    let options = options.unwrap_or_else(Vec::new);
    print_info(&format!(
        "Mounting partition {} at {} with options: {:?}",
        device.name, mount_point, options
    ));
    let result = Exec::cmd("mount")
        .arg(&device.name)
        .arg(mount_point)
        .args(&options)
        .join();
    if result.is_err() {
        if gracefully_fail && user_input_continue_on_mount_failure() {
            print_warning(&format!(
                "Failed to mount partition {} at {}, skipping...",
                device.name, mount_point
            ));
        } else {
            print_error_and_exit(&format!(
                "Failed to mount partition {} at {}",
                device.name, mount_point
            ));
        }
    }
}

fn umount_block_device(mount_point: &str, recursive: bool) {
    let args = if recursive {
        vec!["-R", mount_point]
    } else {
        vec![mount_point]
    };
    print_info(&format!("Unmounting partition at {}", mount_point));
    Exec::cmd("umount")
        .args(&args)
        .join()
        .expect("Failed to unmount block device");
}

fn list_subvolumes(
    device: &block_device::BlockDevice,
    include_dot_snapshots: bool,
) -> Vec<block_device::BTRFSSubVolume> {
    let tmp_dir = TempDir::with_prefix(format!("cachyos-chroot-temp-mount-{}-", &device.uuid))
        .expect("Failed to create temporary directory");
    let tmp_dir = tmp_dir.into_path();
    let mount_point = tmp_dir.to_str().unwrap();

    mount_block_device(device, mount_point, false, None);

    let subvolumes_raw = Exec::cmd("btrfs")
        .args(&["subvolume", "list", "-t", mount_point])
        .capture()
        .expect("Failed to list BTRFS subvolumes")
        .stdout_str();
    let subvolume_lines: Vec<&str> = subvolumes_raw.trim().split("\n").collect();
    let mut subvolumes: Vec<block_device::BTRFSSubVolume> =
        vec![block_device::BTRFSSubVolume::new(
            device.clone(),
            5,
            "/".to_string(),
        )];

    for subvolume in &subvolume_lines[2..] {
        let subvolume_parts: Vec<&str> = subvolume.split_whitespace().collect();

        if subvolume_parts.len() == 4 {
            let subvolume_id = subvolume_parts[0];
            let subvolume_name = subvolume_parts[3];
            if subvolume_name.starts_with(".snapshots") && !include_dot_snapshots {
                continue;
            }
            subvolumes.push(block_device::BTRFSSubVolume::new(
                device.clone(),
                subvolume_id.parse().unwrap(),
                subvolume_name.to_string(),
            ));
        }
    }

    umount_block_device(mount_point, false);

    return subvolumes;
}

fn main() {
    let args = args::Args::parse();

    if !Uid::effective().is_root() && !args.skip_root_check {
        print_error_and_exit(
            "This program must be run as root, to skip this check use --skip-root-check",
        );
    }

    let depends = [
        ("lsblk", "util-linux"),
        ("mount", "util-linux"),
        ("umount", "util-linux"),
        ("arch-chroot", "arch-install-scripts"),
        ("btrfs", "btrfs-progs"),
    ];

    for (cmd, pkg) in &depends {
        if which(cmd).is_err() {
            print_error_and_exit(&format!(
                "Command {} not found, please install {}",
                cmd, pkg
            ));
        }
    }

    let disks_raw = Exec::cmd("lsblk")
        .args(&[
            "-f",
            "-o",
            "NAME,FSTYPE,UUID,PARTUUID,LABEL,PARTLABEL",
            "-p",
            "-a",
            "-J",
            "-Q",
            "type=='part' && fstype!='swap'",
        ])
        .capture()
        .expect("Failed to run lsblk")
        .stdout_str();
    let disks: block_device::BlockDevices =
        serde_json::from_str(&disks_raw).expect("Failed to parse lsblk output");
    let size = disks.block_devices.len();
    print_info(&format!("Found {} block devices", size));

    if size == 0 {
        print_error_and_exit("No block devices found on the system");
    }

    let mut mounted_partitions: Vec<String> = Vec::new();

    for disk in &disks.block_devices {
        print_info(&format!("Found partition: {}", disk.to_string()));
    }

    let selected_device = user_input_block_device("root", disks.block_devices.clone(), false)
        .expect("No block device selected for root partition");
    let mut discovered_btrfs_subvolumes: HashMap<String, Vec<block_device::BTRFSSubVolume>> =
        HashMap::new();
    let mut root_mount_options: Vec<String> = Vec::new();

    if selected_device.fs_type == "btrfs" {
        root_mount_options.push(String::from("-o"));
        print_info("Selected BTRFS partition, mounting and listing subvolumes...");

        let subvolumes = list_subvolumes(&selected_device, args.show_btrfs_dot_snapshots);
        discovered_btrfs_subvolumes.insert(selected_device.name.clone(), subvolumes.clone());

        for subvolume in &subvolumes {
            print_info(&format!("Found subvolume: {}", subvolume.subvolume_name));
        }
        let selected_subvolume = if subvolumes.len() == 1 {
            print_warning("No subvolumes found, using root subvolume");
            subvolumes[0].clone()
        } else {
            user_input_btrfs_subvolume("root", subvolumes)
        };
        mounted_partitions.push(selected_subvolume.get_id());
        root_mount_options.push(format!("subvolid={}", selected_subvolume.subvolume_id));
    } else {
        mounted_partitions.push(selected_device.get_id());
    }

    let tmp_dir = TempDir::with_prefix(format!(
        "cachyos-chroot-root-mount-{}-",
        &selected_device.uuid
    ))
    .expect("Failed to create temporary directory");
    let tmp_dir = tmp_dir.into_path();
    let root_mount_point = tmp_dir.to_str().unwrap();

    mount_block_device(
        &selected_device,
        root_mount_point,
        false,
        Some(root_mount_options),
    );

    let ideal_fstab_path = Path::new(root_mount_point).join("etc").join("fstab");

    if !ideal_fstab_path.exists() {
        print_warning(
            "Unable to find /etc/fstab in the root partition, is this a valid root partition? Good luck fixing that!",
        );
    }

    if !args.no_auto_mount {
        print_info("Auto-mounting block devices based on /etc/fstab...");
        // TODO: Implement auto-mounting based on /etc/fstab
    }

    if user_input_mount_additional_partitions() {
        loop {
            let mount_point = user_input_mount_point();
            if mount_point.eq_ignore_ascii_case("skip") {
                break;
            }
            let selected_device =
                user_input_block_device(&mount_point, disks.block_devices.clone(), true);
            if selected_device.is_none() {
                continue;
            }
            let selected_device = selected_device.unwrap();
            if mounted_partitions.contains(&selected_device.get_id()) {
                print_warning("Partition already mounted, skipping...");
                continue;
            }
            if selected_device.fs_type == "btrfs" {
                let mut mount_options = vec![String::from("-o")];
                let subvolumes = if discovered_btrfs_subvolumes.contains_key(&selected_device.name)
                {
                    discovered_btrfs_subvolumes
                        .get(&selected_device.name)
                        .unwrap()
                        .clone()
                } else {
                    list_subvolumes(&selected_device, args.show_btrfs_dot_snapshots)
                };
                let selected_subvolume = if subvolumes.len() == 1 {
                    print_warning("No subvolumes found, using root subvolume");
                    subvolumes[0].clone()
                } else {
                    user_input_btrfs_subvolume(&mount_point, subvolumes.clone())
                };
                if mounted_partitions.contains(&selected_subvolume.get_id()) {
                    print_warning("Partition already mounted, skipping...");
                    continue;
                }
                mount_options.push(format!("subvolid={}", selected_subvolume.subvolume_id));
                mount_block_device(
                    &selected_subvolume.device,
                    &mount_point,
                    false,
                    Some(mount_options),
                );
                mounted_partitions.push(selected_subvolume.get_id());
                continue;
            }
            mount_block_device(&selected_device, &mount_point, false, None);
            mounted_partitions.push(selected_device.get_id());
        }
    }

    print_info("Chrooting into the configured root partition...");
    print_info("To exit the chroot, type 'exit' or press Ctrl+D");

    Exec::cmd("arch-chroot")
        .arg(root_mount_point)
        .join()
        .expect("Failed to chroot into root partition");

    umount_block_device(root_mount_point, true);
}
