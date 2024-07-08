pub mod args;
pub mod block_device;

use block_device::BlockOrSubvolumeID;

use std::io;
use std::{collections::HashMap, path::Path, process::exit};

use clap::Parser;
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use log::{Level, Metadata, Record};
use nix::unistd::Uid;
use subprocess::Exec;
use tempfile::TempDir;
use which::which;

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

fn print_error_and_exit(msg: &str) {
    log::error!("{msg}");
    exit(1);
}

fn user_input_block_device(
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

fn user_input_btrfs_subvolume(
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

fn basic_user_input_confirm<'a>(prompt_text: &'a str, theme: &'a ColorfulTheme) -> Confirm<'a> {
    Confirm::with_theme(theme)
        .with_prompt(prompt_text)
        .default(false)
        .show_default(false)
        .wait_for_newline(true)
}

fn user_input_mount_additional_partitions() -> bool {
    basic_user_input_confirm(
        "Do you want to mount additional partitions?",
        &ColorfulTheme::default(),
    )
    .interact()
    .unwrap()
}

fn user_input_continue_on_mount_failure() -> bool {
    basic_user_input_confirm(
        "Do you want to skip mounting this partition?",
        &ColorfulTheme::default(),
    )
    .interact()
    .unwrap()
}

fn user_input_mount_point() -> String {
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

fn mount_block_device(
    device: &block_device::BlockDevice,
    mount_point: &str,
    gracefully_fail: bool,
    options: Option<Vec<String>>,
) {
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
        if gracefully_fail && user_input_continue_on_mount_failure() {
            log::warn!(
                "Failed to mount partition {} at {}, skipping...",
                device.name,
                mount_point
            );
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

    init_logger().expect("Failed to initialize logger");

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

    let selected_device = user_input_block_device("root", &disks.block_devices, false)
        .expect("No block device selected for root partition");
    let mut discovered_btrfs_subvolumes: HashMap<String, Vec<block_device::BTRFSSubVolume>> =
        HashMap::new();
    let mut root_mount_options: Vec<String> = Vec::new();

    if selected_device.fs_type == "btrfs" {
        root_mount_options.push(String::from("-o"));
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
            user_input_btrfs_subvolume("root", &subvolumes)
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
    }

    if !args.no_auto_mount {
        // TODO: Implement auto-mounting based on /etc/fstab
    }

    while user_input_mount_additional_partitions() {
        let mount_point = user_input_mount_point();
        if mount_point.eq_ignore_ascii_case("skip") {
            break;
        }
        let actual_mount_point =
            Path::new(root_mount_point).join(mount_point.trim_start_matches('/'));
        let actual_mount_point = actual_mount_point.to_str().unwrap();
        let selected_device = user_input_block_device(&mount_point, &disks.block_devices, true);
        if selected_device.is_none() {
            continue;
        }
        let selected_device = selected_device.unwrap();
        if mounted_partitions.contains(&selected_device.get_id()) {
            log::warn!("Partition already mounted, skipping...");
            continue;
        }
        if selected_device.fs_type == "btrfs" {
            let mut mount_options = vec!["-o".to_owned()];
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
                user_input_btrfs_subvolume(&mount_point, &subvolumes)
            };
            if mounted_partitions.contains(&selected_subvolume.get_id()) {
                log::warn!("Partition already mounted, skipping...");
                continue;
            }
            mount_options.push(format!("subvolid={}", selected_subvolume.subvolume_id));
            mount_block_device(
                &selected_subvolume.device,
                actual_mount_point,
                true,
                Some(mount_options),
            );
            mounted_partitions.push(selected_subvolume.get_id());
            continue;
        }
        mount_block_device(&selected_device, actual_mount_point, true, None);
        mounted_partitions.push(selected_device.get_id());
    }

    log::info!("Chrooting into the configured root partition...");
    log::info!("To exit the chroot, type 'exit' or press Ctrl+D");

    Exec::cmd("arch-chroot")
        .arg(root_mount_point)
        .join()
        .expect("Failed to chroot into root partition");

    umount_block_device(root_mount_point, true);
}

pub fn init_logger() -> Result<(), log::SetLoggerError> {
    log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Info))
}
