pub mod args;
pub mod block_device;
pub mod btrfs;
pub mod depends;
pub mod features;
pub mod logger;
pub mod luks;
pub mod user_input;
pub mod utils;
pub mod zfs;

use crate::block_device::{BlockDeviceUtils, BlockOrSubvolumeID};
use crate::zfs::ZFSDataSetUtils;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use clap::Parser;
use colored::Colorize;
use fstab::FsTab;
use nix::unistd::Uid;
use subprocess::Exec;
use tempfile::TempDir;

fn main() {
    let args = args::Args::parse();

    logger::init_logger().expect("Failed to initialize logger");

    if !Uid::effective().is_root() && !args.skip_root_check {
        utils::print_error_and_exit(
            "This program must be run as root, to skip this check use --skip-root-check",
        );
    }

    if args.skip_root_check {
        log::warn!(
            "Root permission check skipped, make sure you have the necessary permissions to run \
             this program"
        );
    }

    let features = features::get_enabled_features_from_depends();

    let mut block_devices = block_device::list_block_devices(None);
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
    let mut discovered_btrfs_subvolumes: HashMap<String, Vec<btrfs::BTRFSSubVolume>> =
        HashMap::new();
    let mut root_mount_options: Vec<String> = Vec::new();
    let mut opened_luks_devices: Vec<block_device::BlockDevice> = Vec::new();
    let mut imported_zfs_pools: Vec<block_device::BlockDevice> = Vec::new();
    let mut loaded_zfs_keys: HashSet<String> = HashSet::new();
    let mut has_luks_on_root = false;

    if selected_device.is_crypto_luks() {
        if !features.contains(features::Features::LUKS) {
            utils::print_error_and_exit(
                "LUKS encrypted partition selected but LUKS support is disabled or missing \
                 required binaries in PATH",
            );
        }
        has_luks_on_root = true;
        luks::open_device(selected_device);
        opened_luks_devices.push(selected_device.clone());
        block_devices = block_device::list_block_devices(Some(opened_luks_devices.to_owned()));
        selected_device = user_input::get_block_device("root", &block_devices, false)
            .expect("No block device selected for root partition");
    }

    let tmp_dir =
        TempDir::with_prefix(format!("cachyos-chroot-root-mount-{}-", &selected_device.uuid))
            .expect("Failed to create temporary directory");
    let tmp_dir = tmp_dir.keep();
    let root_mount_point = tmp_dir.to_str().unwrap();

    if selected_device.is_btrfs() {
        if !features.contains(features::Features::BTRFS) {
            utils::print_error_and_exit(
                "BTRFS partition selected but BTRFS support is disabled or missing required \
                 binaries in PATH",
            );
        }
        root_mount_options.push("-o".to_owned());
        log::info!("Selected BTRFS partition, mounting and listing subvolumes...");

        let selected_subvolume = btrfs::get_btrfs_subvolume(
            selected_device,
            &mut discovered_btrfs_subvolumes,
            args.show_btrfs_dot_snapshots,
            "root",
        );
        mounted_partitions.push(selected_subvolume.get_id());
        root_mount_options.push(format!("subvolid={}", selected_subvolume.subvolume_id));
    } else if selected_device.is_zfs_member() {
        if !features.contains(features::Features::ZFS) {
            utils::print_error_and_exit(
                "ZFS partition selected but ZFS support is disabled or missing required binaries \
                 in PATH",
            );
        }
        zfs::import_zfs_pool(selected_device, root_mount_point);
        imported_zfs_pools.push(selected_device.clone());
        let mut zfs_datasets =
            zfs::list_zfs_mountable_datasets(selected_device, &mut loaded_zfs_keys);
        if zfs_datasets.is_empty() {
            utils::print_error_and_exit(
                "No mountable ZFS datasets found in the selected partition",
            );
        }
        log::info!("Found {} mountable ZFS datasets", zfs_datasets.len());
        let selection =
            user_input::get_zfs_datasets(&selected_device.get_id(), &zfs_datasets, false);
        for (i, dataset) in zfs_datasets.iter_mut().enumerate() {
            if selection.contains(&i) {
                zfs::mount_zfs_dataset(dataset, root_mount_point, true);
            } else if dataset.is_mounted() {
                zfs::unmount_zfs_dataset(dataset);
            }
        }
        zfs_datasets.iter().for_each(|zfs_dataset| {
            if zfs_dataset.is_mounted() {
                mounted_partitions.push(zfs_dataset.get_id());
            }
        });
    } else {
        mounted_partitions.push(selected_device.get_id());
    }

    if !selected_device.is_zfs_member() {
        block_device::mount_block_device(
            selected_device,
            root_mount_point,
            false,
            Some(root_mount_options),
        );
    }

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
            let device = {
                let crypttab_entry = crypttab_entries.get(&entry.fs_spec);
                block_devices.iter().find(|d| {
                    d.matches_fstab_entry(crypttab_entry.unwrap_or(&entry.fs_spec))
                        || d.matches_fstab_entry(&entry.fs_spec)
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
            if device.is_btrfs() {
                let known_subvolumes = if discovered_btrfs_subvolumes.contains_key(&device.uuid) {
                    discovered_btrfs_subvolumes.get(&device.uuid).unwrap().clone()
                } else {
                    let subvolumes = btrfs::list_subvolumes(device, args.show_btrfs_dot_snapshots);
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
                if block_device::mount_block_device(
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
            if block_device::mount_block_device(device, actual_mount_point, true, None) {
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
        if selected_device.is_crypto_luks() {
            if !features.contains(features::Features::LUKS) {
                utils::print_error_and_exit(
                    "LUKS encrypted partition selected but LUKS support is disabled or missing \
                     required binaries in PATH",
                );
            }
            luks::open_device(selected_device);
            opened_luks_devices.push(selected_device.clone());
            block_devices = block_device::list_block_devices(Some(opened_luks_devices.to_owned()));
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
        if selected_device.is_btrfs() {
            if !features.contains(features::Features::BTRFS) {
                utils::print_error_and_exit(
                    "BTRFS partition selected but BTRFS support is disabled or missing required \
                     binaries in PATH",
                );
            }
            let selected_subvolume = btrfs::get_btrfs_subvolume(
                selected_device,
                &mut discovered_btrfs_subvolumes,
                args.show_btrfs_dot_snapshots,
                &mount_point,
            );
            if mounted_partitions.contains(&selected_subvolume.get_id()) {
                log::warn!("Partition already mounted, skipping...");
                continue;
            }
            if block_device::mount_block_device(
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
        } else if selected_device.is_zfs_member() {
            if !features.contains(features::Features::ZFS) {
                utils::print_error_and_exit(
                    "ZFS partition selected but ZFS support is disabled or missing required \
                     binaries in PATH",
                );
            }
            if imported_zfs_pools.iter().any(|d| d.get_id() == selected_device.get_id()) {
                log::info!(
                    "ZFS pool for the selected partition is already imported, proceeding to \
                     dataset selection..."
                );
            } else {
                zfs::import_zfs_pool(selected_device, root_mount_point);
                imported_zfs_pools.push(selected_device.clone());
            }
            let mut zfs_datasets =
                zfs::list_zfs_mountable_datasets(selected_device, &mut loaded_zfs_keys);
            if zfs_datasets.is_empty() {
                log::warn!(
                    "No mountable ZFS datasets found in the selected partition, skipping..."
                );
                continue;
            }
            log::info!("Found {} mountable ZFS datasets", zfs_datasets.len());
            let selection =
                user_input::get_zfs_datasets(&selected_device.get_id(), &zfs_datasets, true);
            for (i, dataset) in zfs_datasets.iter_mut().enumerate() {
                if selection.contains(&i) {
                    zfs::mount_zfs_dataset(dataset, actual_mount_point, true);
                } else if dataset.is_mounted() {
                    zfs::unmount_zfs_dataset(dataset);
                }
            }
            zfs_datasets.iter().for_each(|zfs_dataset| {
                if zfs_dataset.is_mounted() {
                    mounted_partitions.push(zfs_dataset.get_id());
                } else {
                    mounted_partitions.retain(|d| d != &zfs_dataset.get_id());
                }
            });
            continue;
        }
        if block_device::mount_block_device(selected_device, actual_mount_point, true, None) {
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

    block_device::umount_block_device(root_mount_point, true);
    for device in opened_luks_devices {
        luks::close_device(&device);
    }
    for key in loaded_zfs_keys {
        zfs::unload_zfs_key(&key);
    }
    for device in imported_zfs_pools {
        zfs::export_zfs_pool(&device);
    }
}
