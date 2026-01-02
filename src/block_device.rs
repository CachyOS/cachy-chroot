use crate::{block_device, user_input, utils};

use serde::{Deserialize, Serialize};
use subprocess::Exec;

pub trait BlockOrSubvolumeID {
    fn get_id(&self) -> String;
}

pub trait BlockDeviceUtils {
    fn is_crypto_luks(&self) -> bool;
    fn is_zfs_member(&self) -> bool;
    fn is_btrfs(&self) -> bool;
    fn matches_fstab_entry(&self, id: &str) -> bool;
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BlockDevice {
    pub name: String,
    #[serde(rename = "fstype")]
    pub fs_type: String,
    pub uuid: String,
    pub partuuid: Option<String>,
    pub label: Option<String>,
    pub partlabel: Option<String>,
}

impl std::fmt::Display for BlockDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Partition: {}: FS: {} UUID: {}", self.name, self.fs_type, self.uuid)
    }
}

impl BlockOrSubvolumeID for BlockDevice {
    fn get_id(&self) -> String {
        if self.fs_type == "zfs_member"
            && let Some(label) = &self.label
        {
            return label.clone();
        }
        self.uuid.clone()
    }
}

impl BlockDeviceUtils for BlockDevice {
    fn is_crypto_luks(&self) -> bool {
        self.fs_type.eq_ignore_ascii_case("crypto_LUKS")
    }

    fn is_zfs_member(&self) -> bool {
        self.fs_type.eq_ignore_ascii_case("zfs_member")
    }

    fn is_btrfs(&self) -> bool {
        self.fs_type.eq_ignore_ascii_case("btrfs")
    }

    fn matches_fstab_entry(&self, id: &str) -> bool {
        if let Some(uuid) = id.strip_prefix("UUID=") {
            return uuid == self.uuid;
        } else if let Some(uuid) = id.strip_prefix("/dev/disk/by-uuid/") {
            return uuid == self.uuid;
        } else if let Some(partuuid) = id.strip_prefix("PARTUUID=")
            && let Some(device_partuuid) = self.partuuid.clone()
        {
            return partuuid == device_partuuid;
        } else if let Some(partuuid) = id.strip_prefix("/dev/disk/by-partuuid/")
            && let Some(device_partuuid) = self.partuuid.clone()
        {
            return partuuid == device_partuuid;
        } else if let Some(label) = id.strip_prefix("LABEL=")
            && let Some(device_label) = self.label.clone()
        {
            return label == device_label;
        } else if let Some(partlabel) = id.strip_prefix("PARTLABEL=")
            && let Some(device_partlabel) = self.partlabel.clone()
        {
            return partlabel == device_partlabel;
        }
        id == self.name
    }
}

#[derive(Serialize, Deserialize)]
pub struct BlockDevices {
    #[serde(rename = "blockdevices")]
    pub block_devices: Vec<BlockDevice>,
}

pub fn mount_block_device(
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

pub fn umount_block_device(mount_point: &str, recursive: bool) {
    let args = if recursive { vec!["-R", mount_point] } else { vec![mount_point] };
    log::info!("Unmounting partition at {}", mount_point);
    Exec::cmd("umount").args(&args).join().expect("Failed to unmount block device");
}

pub fn list_block_devices(ignored_devices: Option<Vec<BlockDevice>>) -> Vec<BlockDevice> {
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
