pub mod args;
pub mod block_device;
pub mod logger;
pub mod user_input;

use block_device::BlockOrSubvolumeID;

use std::{collections::HashMap, path::Path, process::exit};

use clap::Parser;
use colored::Colorize;
use fstab::FsTab;
use nix::unistd::Uid;
use subprocess::Exec;
use tempfile::TempDir;
use which::which;

fn print_error_and_exit(msg: &str) {
    log::error!("{msg}");
    exit(1);
}

fn mount_block_device(
    device: &block_device::BlockDevice,
    mount_point: &str,
    gracefully_fail: bool,
    options: Option<Vec<String>>,
) -> bool {
    let options = options.unwrap_or_default();
    log::info!(
        "Mounting partition {} at {} with options: {:?}",
        device.name,
        mount_point,
        options
    );
    let result = Exec::cmd("mount")
        .arg(&device.name)
        .arg(mount_point)
        .args(&options)
        .join();
    if result.is_err() || !result.unwrap().success() {
        if gracefully_fail && user_input::continue_on_mount_failure() {
            log::warn!(
                "Failed to mount partition {} at {}, skipping...",
                device.name,
                mount_point
            );
            return false;
        } else {
            print_error_and_exit(&format!(
                "Failed to mount partition {} at {}",
                device.name, mount_point
            ));
        }
    }
    true
}

fn umount_block_device(mount_point: &str, recursive: bool) {
    let args = if recursive {
        vec!["-R", mount_point]
    } else {
        vec![mount_point]
    };
    log::info!("Unmounting partition at {}", mount_point);
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
    let subvolume_lines = subvolumes_raw.trim().split('\n').collect::<Vec<_>>();
    let mut subvolumes = vec![block_device::BTRFSSubVolume {
        device: device.clone(),
        subvolume_id: 5,
        subvolume_name: "/".to_owned(),
    }];

    for subvolume in &subvolume_lines[2..] {
        let subvolume_parts = subvolume.split_whitespace().collect::<Vec<_>>();

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

    subvolumes
}

fn main() {
    let args = args::Args::parse();

    logger::init_logger().expect("Failed to initialize logger");

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
    log::info!("Found {} block devices", size);

    if size == 0 {
        print_error_and_exit("No block devices found on the system");
    }

    let mut mounted_partitions: Vec<String> = Vec::new();

    for disk in &disks.block_devices {
        log::info!("Found partition: {}", disk.to_string());
    }

    let selected_device = user_input::get_block_device("root", &disks.block_devices, false)
        .expect("No block device selected for root partition");
    let mut discovered_btrfs_subvolumes: HashMap<String, Vec<block_device::BTRFSSubVolume>> =
        HashMap::new();
    let mut root_mount_options: Vec<String> = Vec::new();

    if selected_device.fs_type == "btrfs" {
        root_mount_options.push("-o".to_owned());
        log::info!("Selected BTRFS partition, mounting and listing subvolumes...");

        let subvolumes = list_subvolumes(&selected_device, args.show_btrfs_dot_snapshots);
        discovered_btrfs_subvolumes.insert(selected_device.name.clone(), subvolumes.clone());

        for subvolume in &subvolumes {
            log::info!("Found subvolume: {}", subvolume.subvolume_name);
        }
        let selected_subvolume = if subvolumes.len() == 1 {
            log::warn!("No subvolumes found, using root subvolume");
            subvolumes[0].clone()
        } else {
            let cachy_default_root_subvol = subvolumes
                .iter()
                .find(|subvol| subvol.subvolume_name == "@");
            if cachy_default_root_subvol.is_some() && user_input::use_cachyos_btrfs_preset() {
                cachy_default_root_subvol.unwrap().clone()
            } else {
                user_input::get_btrfs_subvolume("root", &subvolumes)
            }
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
        log::warn!(
            "Unable to find /etc/fstab in the root partition, is this a valid root partition? Good luck fixing that!",
        );
    } else if !args.no_auto_mount {
        log::info!("Mounting additional partitions based on /etc/fstab...");
        let fstab = FsTab::new(&ideal_fstab_path);
        let entries = fstab.get_entries().unwrap_or_default();
        log::info!("Found {} entries in /etc/fstab", entries.len());
        for entry in &entries {
            if entry.vfs_type == "swap" {
                continue;
            }
            let device = if entry.fs_spec.starts_with("/dev") {
                disks.block_devices.iter().find(|d| d.name == entry.fs_spec)
            } else {
                let fs_spec = entry.fs_spec.split('=').collect::<Vec<_>>();
                if fs_spec.len() != 2 {
                    log::warn!("Invalid fs_spec in fstab, skipping...");
                    continue;
                }
                let fs_spec = fs_spec.last().unwrap();
                disks.block_devices.iter().find(|d| {
                    d.uuid == *fs_spec
                        || d.partuuid == *fs_spec
                        || d.label == Some(fs_spec.to_string())
                        || d.partlabel == Some(fs_spec.to_string())
                })
            };
            if device.is_none() {
                log::warn!(
                    "Device {} not found, skipping mounting...",
                    entry.fs_spec.yellow()
                );
                continue;
            }
            let device = device.unwrap();
            if mounted_partitions.contains(&device.get_id()) {
                log::warn!(
                    "Partition {} already mounted, skipping...",
                    entry.fs_spec.yellow()
                );
                continue;
            }
            let actual_mount_point = Path::new(root_mount_point)
                .join(entry.mountpoint.to_str().unwrap().trim_start_matches('/'));
            let actual_mount_point = actual_mount_point.to_str().unwrap();
            if device.fs_type == "btrfs" {
                let known_subvolumes = if discovered_btrfs_subvolumes.contains_key(&device.name) {
                    discovered_btrfs_subvolumes
                        .get(&device.name)
                        .unwrap()
                        .clone()
                } else {
                    let subvolumes = list_subvolumes(device, args.show_btrfs_dot_snapshots);
                    discovered_btrfs_subvolumes.insert(device.name.clone(), subvolumes.clone());
                    subvolumes
                };
                let fstab_opt_subvolume_id: Option<usize> =
                    entry.mount_options.iter().find_map(|opt| {
                        if opt.starts_with("subvolid=") {
                            Some(opt.trim_start_matches("subvolid=").parse().unwrap())
                        } else {
                            None
                        }
                    });
                let fstab_opt_subvolume: Option<String> =
                    entry.mount_options.iter().find_map(|opt| {
                        if opt.starts_with("subvol=") {
                            Some(opt.trim_start_matches("subvol=").to_string())
                        } else {
                            None
                        }
                    });
                let selected_subvolume = if let Some(subvolume_id) = fstab_opt_subvolume_id {
                    known_subvolumes
                        .iter()
                        .find(|subvol| subvol.subvolume_id == subvolume_id)
                } else if let Some(subvolume_name) = fstab_opt_subvolume {
                    known_subvolumes.iter().find(|subvol| {
                        subvol.subvolume_name == subvolume_name
                            || subvolume_name.strip_prefix('/').unwrap() == subvol.subvolume_name
                    })
                } else {
                    log::warn!("No subvolume specified in fstab, using root subvolume");
                    Some(&known_subvolumes[0])
                };
                if selected_subvolume.is_none() {
                    log::warn!(
                        "No subvolume found for entry: {} {}, skipping...",
                        entry.fs_spec,
                        entry.mountpoint.to_str().unwrap()
                    );
                    continue;
                }
                let selected_subvolume = selected_subvolume.unwrap();
                if mounted_partitions.contains(&selected_subvolume.get_id()) {
                    log::warn!(
                        "Partition already mounted: {} {}, skipping...",
                        entry.fs_spec,
                        entry.mountpoint.to_str().unwrap()
                    );
                    continue;
                }
                if mount_block_device(
                    &selected_subvolume.device,
                    actual_mount_point,
                    true,
                    Some(vec![
                        "-o".to_owned(),
                        format!("subvolid={}", selected_subvolume.subvolume_id),
                    ]),
                ) {
                    mounted_partitions.push(selected_subvolume.get_id());
                }
                continue;
            }
            if mount_block_device(device, actual_mount_point, true, None) {
                mounted_partitions.push(device.get_id());
            }
        }
        log::info!("Finished mounting additional partitions");
    }

    while user_input::mount_additional_partitions() {
        let mount_point = user_input::get_mount_point();
        if mount_point.eq_ignore_ascii_case("skip") {
            break;
        }
        let actual_mount_point =
            Path::new(root_mount_point).join(mount_point.trim_start_matches('/'));
        let actual_mount_point = actual_mount_point.to_str().unwrap();
        let selected_device =
            user_input::get_block_device(&mount_point, &disks.block_devices, true);
        if selected_device.is_none() {
            continue;
        }
        let selected_device = selected_device.unwrap();
        if mounted_partitions.contains(&selected_device.get_id()) {
            log::warn!("Partition already mounted, skipping...");
            continue;
        }
        if selected_device.fs_type == "btrfs" {
            let subvolumes = if discovered_btrfs_subvolumes.contains_key(&selected_device.name) {
                discovered_btrfs_subvolumes
                    .get(&selected_device.name)
                    .unwrap()
                    .clone()
            } else {
                list_subvolumes(&selected_device, args.show_btrfs_dot_snapshots)
            };
            let selected_subvolume = if subvolumes.len() == 1 {
                log::warn!("No subvolumes found, using root subvolume");
                subvolumes[0].clone()
            } else {
                user_input::get_btrfs_subvolume(&mount_point, &subvolumes)
            };
            if mounted_partitions.contains(&selected_subvolume.get_id()) {
                log::warn!("Partition already mounted, skipping...");
                continue;
            }
            if mount_block_device(
                &selected_subvolume.device,
                actual_mount_point,
                true,
                Some(vec![
                    "-o".to_owned(),
                    format!("subvolid={}", selected_subvolume.subvolume_id),
                ]),
            ) {
                mounted_partitions.push(selected_subvolume.get_id());
            }
            continue;
        }
        if mount_block_device(&selected_device, actual_mount_point, true, None) {
            mounted_partitions.push(selected_device.get_id());
        }
    }

    log::info!("Chrooting into the configured root partition...");
    log::info!("To exit the chroot, type 'exit' or press Ctrl+D");

    Exec::cmd("arch-chroot")
        .arg(root_mount_point)
        .join()
        .expect("Failed to chroot into root partition");

    umount_block_device(root_mount_point, true);
}
