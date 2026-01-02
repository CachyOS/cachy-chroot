use crate::block_device::*;
use crate::user_input;

use std::collections::HashMap;
use subprocess::Exec;
use tempfile::TempDir;

#[derive(Clone)]
pub struct BTRFSSubVolume {
    pub device: BlockDevice,
    pub subvolume_id: usize,
    pub subvolume_name: String,
}

impl BTRFSSubVolume {
    pub fn new(device: BlockDevice, subvolume_id: usize, subvolume_name: String) -> Self {
        BTRFSSubVolume { device, subvolume_id, subvolume_name }
    }
}

impl std::fmt::Display for BTRFSSubVolume {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "[{}] BTRFS Subvolume: {}: SubVol ID: {}",
            self.device.name, self.subvolume_name, self.subvolume_id
        )
    }
}

impl BlockOrSubvolumeID for BTRFSSubVolume {
    fn get_id(&self) -> String {
        format!("{}-{}", self.device.get_id(), self.subvolume_id)
    }
}

pub fn list_subvolumes(device: &BlockDevice, include_dot_snapshots: bool) -> Vec<BTRFSSubVolume> {
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

pub fn get_btrfs_subvolume(
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
        if let Some(cachy_default_root_subvol) =
            known_subvolumes.iter().find(|subvol| subvol.subvolume_name == "@")
            && user_input::use_cachyos_btrfs_preset()
        {
            cachy_default_root_subvol.clone()
        } else {
            user_input::get_btrfs_subvolume(device_name, &known_subvolumes)
        }
    } else {
        user_input::get_btrfs_subvolume(device_name, &known_subvolumes)
    }
}
