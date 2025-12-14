pub mod args;
pub mod block_device;
pub mod logger;
pub mod luks;
pub mod user_input;
pub mod utils;

use block_device::{BTRFSSubVolume, BlockDevice, BlockOrSubvolumeID};

use std::collections::HashMap;
use std::path::Path;

use clap::Parser;
use colored::Colorize;
use fstab::FsTab;
use nix::unistd::Uid;
use subprocess::Exec;
use tempfile::TempDir;
use which::which;

fn mount_block_device(
    device: &BlockDevice,
    mount_point: &str,
    gracefully_fail: bool,
    options: Option<Vec<String>>,
) -> bool {
    let options = options.unwrap_or_default();
    log::info!("Mounting partition {} at {} with options: {:?}", device.name, mount_point, options);
    let result = Exec::cmd("mount").arg(&device.name).arg(mount_point).args(&options).join();
    if result.is_err() || !result.unwrap().success() {
        if gracefully_fail && user_input::continue_on_mount_failure() {
            log::warn!("Failed to mount partition {} at {}, skipping...", device.name, mount_point);
            return false;
        } else {
            utils::print_error_and_exit(&format!(
                "Failed to mount partition {} at {}",
                device.name, mount_point
            ));
        }
    }
    true
}

fn umount_block_device(mount_point: &str, recursive: bool) {
    let args = if recursive { vec!["-R", mount_point] } else { vec![mount_point] };
    log::info!("Unmounting partition at {}", mount_point);
    Exec::cmd("umount").args(&args).join().expect("Failed to unmount block device");
}

fn list_subvolumes(device: &BlockDevice, include_dot_snapshots: bool) -> Vec<BTRFSSubVolume> {
    let tmp_dir = TempDir::with_prefix(format!("cachyos-chroot-temp-mount-{}-", &device.uuid))
        .expect("Failed to create temporary directory");
    let tmp_dir = tmp_dir.keep();
    let mount_point = tmp_dir.to_str().unwrap();

    mount_block_device(device, mount_point, false, None);

    let subvolumes_raw = Exec::cmd("btrfs")
        .args(&["subvolume", "list", "-t", mount_point])
        .capture()
        .expect("Failed to list BTRFS subvolumes")
        .stdout_str();
    let subvolume_lines = subvolumes_raw.trim().split('\n').collect::<Vec<_>>();
    let mut subvolumes = vec![BTRFSSubVolume {
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
            subvolumes.push(BTRFSSubVolume::new(
                device.clone(),
                subvolume_id.parse().unwrap(),
                subvolume_name.to_string(),
            ));
        }
    }

    umount_block_device(mount_point, false);

    subvolumes
}

fn get_btrfs_subvolume(
    device: &BlockDevice,
    discovered_btrfs_subvolumes: &mut HashMap<String, Vec<BTRFSSubVolume>>,
    show_btrfs_dot_snapshots: bool,
    device_name: &str,
) -> BTRFSSubVolume {
    let known_subvolumes = if discovered_btrfs_subvolumes.contains_key(&device.uuid) {
        discovered_btrfs_subvolumes.get(&device.uuid).unwrap().clone()
    } else {
        let subvolumes = list_subvolumes(device, show_btrfs_dot_snapshots);
        discovered_btrfs_subvolumes.insert(device.uuid.clone(), subvolumes.clone());
        subvolumes
    };
    if known_subvolumes.len() == 1 {
        log::warn!("No subvolumes found, using root subvolume");
        known_subvolumes[0].clone()
    } else if device_name == "root" {
        let cachy_default_root_subvol =
            known_subvolumes.iter().find(|subvol| subvol.subvolume_name == "@");
        if cachy_default_root_subvol.is_some() && user_input::use_cachyos_btrfs_preset() {
            cachy_default_root_subvol.unwrap().clone()
        } else {
            user_input::get_btrfs_subvolume(device_name, &known_subvolumes)
        }
    } else {
        user_input::get_btrfs_subvolume(device_name, &known_subvolumes)
    }
}

fn list_block_devices(ignored_devices: Option<Vec<BlockDevice>>) -> Vec<BlockDevice> {
    let disks_raw = Exec::cmd("lsblk")
        .args(&[
            "-f",
            "-o",
            "NAME,FSTYPE,UUID,PARTUUID,LABEL,PARTLABEL",
            "-p",
            "-a",
            "-J",
            "-Q",
            "type=='part' || type=='crypt' && fstype!='swap' && fstype && uuid",
        ])
        .capture()
        .expect("Failed to run lsblk")
        .stdout_str();

    let disks: block_device::BlockDevices =
        serde_json::from_str(&disks_raw).expect("Failed to parse lsblk output");

    let ignored_devices = ignored_devices.unwrap_or_default();
    let block_devices = disks.block_devices;

    if ignored_devices.is_empty() {
        return block_devices;
    }

    block_devices.into_iter().filter(|d| !ignored_devices.contains(d)).collect()
}

fn main() {
    let args = args::Args::parse();

    logger::init_logger().expect("Failed to initialize logger");

    if !Uid::effective().is_root() && !args.skip_root_check {
        utils::print_error_and_exit(
            "This program must be run as root, to skip this check use --skip-root-check",
        );
    }

    let depends = [
        ("lsblk", "util-linux"),
        ("mount", "util-linux"),
        ("umount", "util-linux"),
        ("arch-chroot", "arch-install-scripts"),
        ("btrfs", "btrfs-progs"),
        ("cryptsetup", "cryptsetup"),
    ];

    for (cmd, pkg) in &depends {
        if which(cmd).is_err() {
            utils::print_error_and_exit(&format!(
                "Command {} not found, please install {}",
                cmd, pkg
            ));
        }
    }

    let mut block_devices = list_block_devices(None);
    let size = block_devices.len();
    log::info!("Found {} block devices", size);

    if size == 0 {
        utils::print_error_and_exit("No block devices found on the system");
    }

    let mut mounted_partitions: Vec<String> = Vec::new();

    for disk in &block_devices {
        log::info!("Found partition: {}", disk);
    }

    let mut selected_device = user_input::get_block_device("root", &block_devices, false)
        .expect("No block device selected for root partition");
    let mut discovered_btrfs_subvolumes: HashMap<String, Vec<BTRFSSubVolume>> = HashMap::new();
    let mut root_mount_options: Vec<String> = Vec::new();
    let mut opened_luks_devices: Vec<BlockDevice> = Vec::new();
    let mut has_luks_on_root = false;

    if selected_device.fs_type == "crypto_LUKS" {
        has_luks_on_root = true;
        luks::open_device(selected_device);
        opened_luks_devices.push(selected_device.clone());
        block_devices = list_block_devices(Some(opened_luks_devices.to_owned()));
        selected_device = user_input::get_block_device("root", &block_devices, false)
            .expect("No block device selected for root partition");
    }

    if selected_device.fs_type == "btrfs" {
        root_mount_options.push("-o".to_owned());
        log::info!("Selected BTRFS partition, mounting and listing subvolumes...");

        let selected_subvolume = get_btrfs_subvolume(
            selected_device,
            &mut discovered_btrfs_subvolumes,
            args.show_btrfs_dot_snapshots,
            "root",
        );
        mounted_partitions.push(selected_subvolume.get_id());
        root_mount_options.push(format!("subvolid={}", selected_subvolume.subvolume_id));
    } else {
        mounted_partitions.push(selected_device.get_id());
    }

    let tmp_dir =
        TempDir::with_prefix(format!("cachyos-chroot-root-mount-{}-", &selected_device.uuid))
            .expect("Failed to create temporary directory");
    let tmp_dir = tmp_dir.keep();
    let root_mount_point = tmp_dir.to_str().unwrap();

    mount_block_device(selected_device, root_mount_point, false, Some(root_mount_options));

    let ideal_fstab_path = Path::new(root_mount_point).join("etc").join("fstab");
    let ideal_crypttab_path = Path::new(root_mount_point).join("etc").join("crypttab");

    let crypttab_entries = luks::list_crypttab_entries(&ideal_crypttab_path, has_luks_on_root);

    if !ideal_fstab_path.exists() {
        log::warn!(
            "Unable to find /etc/fstab in the root partition, is this a valid root partition? \
             Good luck fixing that!",
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
                let crypttab_entry = crypttab_entries.get(&entry.fs_spec);
                block_devices.iter().find(|d| {
                    crypttab_entry == Some(&d.name)
                        || crypttab_entry == Some(&d.uuid)
                        || d.name == entry.fs_spec
                })
            } else {
                let fs_spec = entry.fs_spec.split('=').collect::<Vec<_>>();
                if fs_spec.len() != 2 {
                    log::warn!("Invalid fs_spec in fstab, skipping...");
                    continue;
                }
                let fs_spec = fs_spec.last().unwrap();
                block_devices.iter().find(|d| {
                    d.uuid == *fs_spec
                        || d.partuuid == Some(fs_spec.to_string())
                        || d.label == Some(fs_spec.to_string())
                        || d.partlabel == Some(fs_spec.to_string())
                })
            };
            if device.is_none() {
                log::warn!("Device {} not found, skipping mounting...", entry.fs_spec.yellow());
                continue;
            }
            let device = device.unwrap();
            if mounted_partitions.contains(&device.get_id()) {
                log::warn!("Partition {} already mounted, skipping...", entry.fs_spec.yellow());
                continue;
            }
            let actual_mount_point = Path::new(root_mount_point)
                .join(entry.mountpoint.to_str().unwrap().trim_start_matches('/'));
            let actual_mount_point = actual_mount_point.to_str().unwrap();
            if device.fs_type == "btrfs" {
                let known_subvolumes = if discovered_btrfs_subvolumes.contains_key(&device.uuid) {
                    discovered_btrfs_subvolumes.get(&device.uuid).unwrap().clone()
                } else {
                    let subvolumes = list_subvolumes(device, args.show_btrfs_dot_snapshots);
                    discovered_btrfs_subvolumes.insert(device.uuid.clone(), subvolumes.clone());
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
                    known_subvolumes.iter().find(|subvol| subvol.subvolume_id == subvolume_id)
                } else if let Some(subvolume_name) = fstab_opt_subvolume {
                    known_subvolumes.iter().find(|subvol| {
                        subvol.subvolume_name == subvolume_name
                            || subvolume_name.strip_prefix('/').unwrap_or_default()
                                == subvol.subvolume_name
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
        let selected_device = user_input::get_block_device(&mount_point, &block_devices, true);
        if selected_device.is_none() {
            continue;
        }
        let mut selected_device = selected_device.unwrap();
        if selected_device.fs_type == "crypto_LUKS" {
            luks::open_device(selected_device);
            opened_luks_devices.push(selected_device.clone());
            block_devices = list_block_devices(Some(opened_luks_devices.to_owned()));
            let user_selection = user_input::get_block_device(&mount_point, &block_devices, true);
            if user_selection.is_none() {
                continue;
            }
            selected_device = user_selection.unwrap();
        }
        if mounted_partitions.contains(&selected_device.get_id()) {
            log::warn!("Partition already mounted, skipping...");
            continue;
        }
        if selected_device.fs_type == "btrfs" {
            let selected_subvolume = get_btrfs_subvolume(
                selected_device,
                &mut discovered_btrfs_subvolumes,
                args.show_btrfs_dot_snapshots,
                &mount_point,
            );
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
        if mount_block_device(selected_device, actual_mount_point, true, None) {
            mounted_partitions.push(selected_device.get_id());
        }
    }

    log::info!("Chrooting into the configured root partition...");
    log::info!("To exit the chroot, type 'exit' or press Ctrl+D");

    let mount_options =
        if args.no_systemd_chroot { vec![root_mount_point] } else { vec!["-S", root_mount_point] };

    Exec::cmd("arch-chroot")
        .args(&mount_options)
        .join()
        .expect("Failed to chroot into root partition");

    umount_block_device(root_mount_point, true);
    for device in opened_luks_devices {
        luks::close_device(&device);
    }
}
