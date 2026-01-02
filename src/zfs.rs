use crate::block_device::*;
use crate::{user_input, utils};

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use subprocess::Exec;

pub trait ZFSDataSetUtils {
    fn has_unsupported_encryption(&self) -> bool;
    fn is_encrypted(&self) -> bool;
    fn is_mountable(&self) -> bool;
    fn is_mounted(&self) -> bool;
    fn is_valid_key_root(&self) -> bool;
    fn mark_as_mounted(&mut self);
    fn mark_as_unmounted(&mut self);
}

#[derive(Serialize, Deserialize)]
pub struct ZFSDatasets {
    pub datasets: HashMap<String, ZFSDataSet>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ZFSDataSet {
    pub name: String,
    #[serde(rename = "type")]
    pub dataset_type: String,
    pub pool: String,
    pub properties: ZFSProperties,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ZFSProperties {
    pub canmount: ZFSProperty,
    pub encryption: ZFSProperty,
    pub keylocation: ZFSProperty,
    pub mounted: ZFSProperty,
    pub mountpoint: ZFSProperty,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ZFSProperty {
    pub value: String,
}

impl std::fmt::Display for ZFSDataSet {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "ZFS Dataset: {}: Pool: {}, Mountpoint: {}",
            self.name, self.pool, self.properties.mountpoint.value
        )
    }
}

impl ZFSDataSetUtils for ZFSDataSet {
    fn has_unsupported_encryption(&self) -> bool {
        !self.properties.keylocation.value.eq_ignore_ascii_case("none")
            && !self.properties.keylocation.value.eq_ignore_ascii_case("prompt")
    }

    fn is_encrypted(&self) -> bool {
        !self.properties.encryption.value.eq_ignore_ascii_case("off")
    }

    fn is_mountable(&self) -> bool {
        self.dataset_type.eq_ignore_ascii_case("filesystem")
            && !self.properties.canmount.value.eq_ignore_ascii_case("off")
            && !self.properties.mountpoint.value.eq_ignore_ascii_case("none")
    }

    fn is_mounted(&self) -> bool {
        self.properties.mounted.value.eq_ignore_ascii_case("yes")
    }

    fn is_valid_key_root(&self) -> bool {
        self.properties.keylocation.value.eq_ignore_ascii_case("prompt")
    }

    fn mark_as_mounted(&mut self) {
        self.properties.mounted.value = "yes".to_string();
    }

    fn mark_as_unmounted(&mut self) {
        self.properties.mounted.value = "no".to_string();
    }
}

impl BlockOrSubvolumeID for ZFSDataSet {
    fn get_id(&self) -> String {
        format!("{}-{}", self.name, self.pool)
    }
}

pub fn import_zfs_pool(device: &BlockDevice, mount_point: &str) {
    let pool_name = device.get_id();
    log::info!("Importing ZFS pool: {} at: {}", pool_name, mount_point);
    let result =
        Exec::cmd("zpool").arg("import").arg(&device.uuid).arg("-R").arg(mount_point).join();
    if result.is_err() || !result.unwrap().success() {
        if user_input::allow_zfs_forced_import() {
            log::info!("Forcing ZFS pool import...");
            let force_result = Exec::cmd("zpool")
                .arg("import")
                .arg(&device.uuid)
                .arg("-f")
                .arg("-R")
                .arg(mount_point)
                .join();
            if force_result.is_err() || !force_result.unwrap().success() {
                utils::print_error_and_exit(&format!("Failed to import ZFS pool: {}", &pool_name));
            }
        } else {
            utils::print_error_and_exit(&format!("Failed to import ZFS pool: {}", &pool_name));
        }
    }
}

pub fn export_zfs_pool(device: &BlockDevice) {
    let pool_name = device.get_id();
    log::info!("Exporting ZFS pool: {}", &pool_name);
    let result = Exec::cmd("zpool").arg("export").arg(&pool_name).join();
    if result.is_err() || !result.unwrap().success() {
        if user_input::allow_zfs_forced_export() {
            log::info!("Forcing ZFS pool export...");
            let force_result = Exec::cmd("zpool").arg("export").arg("-f").arg(&pool_name).join();
            if force_result.is_err() || !force_result.unwrap().success() {
                utils::print_error_and_exit(&format!(
                    "Failed to export ZFS pool: {}, please perform the operation manually.",
                    &pool_name
                ));
            }
        } else {
            utils::print_error_and_exit(&format!(
                "Failed to export ZFS pool: {}, please perform the operation manually.",
                &pool_name
            ));
        }
    }
}

pub fn unload_zfs_key(dataset: &str) {
    log::info!("Unloading key for ZFS dataset: {}", dataset);
    let result = Exec::cmd("zfs").arg("unload-key").arg(dataset).join();
    if result.is_err() || !result.unwrap().success() {
        log::error!(
            "Failed to unload key for ZFS dataset: {}, please perform the operation manually.",
            dataset
        );
    }
}

pub fn load_zfs_key(dataset: &str) -> bool {
    log::info!("Loading key for ZFS dataset: {}", dataset);
    let mut success = true;
    let result = Exec::cmd("zfs").arg("load-key").arg(dataset).join();
    if result.is_err() || !result.unwrap().success() {
        log::error!("Failed to load key for ZFS dataset: {}", dataset);
        success = false;
    }
    while !success && user_input::retry_zfs_passphrase(dataset) {
        log::info!("Retrying to load key for ZFS dataset: {}", dataset);
        let retry_result = Exec::cmd("zfs").arg("load-key").arg(dataset).join();
        if retry_result.is_err() || !retry_result.unwrap().success() {
            log::error!("Failed to load key for ZFS dataset: {}", dataset);
        } else {
            success = true;
        }
    }
    success
}

pub fn mount_zfs_dataset(dataset: &mut ZFSDataSet, mount_point: &str, gracefully_fail: bool) {
    log::info!("Mounting ZFS dataset {} at {}", dataset.name, mount_point);
    if dataset.is_mounted() {
        log::warn!("ZFS dataset {} is already mounted, skipping...", dataset.name);
        return;
    }
    let result = Exec::cmd("zfs").arg("mount").arg(&dataset.name).join();
    if result.is_err() || !result.unwrap().success() {
        if gracefully_fail && user_input::continue_on_mount_failure() {
            log::warn!(
                "Failed to mount ZFS dataset {} at {}, skipping...",
                dataset.name,
                mount_point
            );
            return;
        } else {
            utils::print_error_and_exit(&format!(
                "Failed to mount ZFS dataset {} at {}",
                dataset.name, mount_point
            ));
        }
    }
    dataset.mark_as_mounted();
}

pub fn unmount_zfs_dataset(dataset: &mut ZFSDataSet) {
    log::info!("Unmounting ZFS dataset {}", dataset.name);
    let result = Exec::cmd("zfs").arg("unmount").arg(&dataset.name).join();
    if result.is_err() || !result.unwrap().success() {
        log::warn!(
            "Failed to unmount ZFS dataset: {}, please perform the operation manually.",
            dataset.name
        );
    } else {
        dataset.mark_as_unmounted();
    }
}

pub fn list_zfs_mountable_datasets(
    device: &BlockDevice,
    loaded_keys: &mut HashSet<String>,
) -> Vec<ZFSDataSet> {
    let zfs_datasets_raw = Exec::cmd("zfs")
        .args(&[
            "list",
            "-j",
            "-o",
            "canmount,encryption,keylocation,mounted,mountpoint",
            "-t",
            "filesystem",
            "-r",
            &device.get_id(),
        ])
        .capture()
        .expect("Failed to list ZFS datasets")
        .stdout_str();
    let datasets: ZFSDatasets =
        serde_json::from_str(&zfs_datasets_raw).expect("Failed to parse zfs list output");
    if datasets.datasets.values().any(|ds| ds.has_unsupported_encryption()) {
        log::warn!(
            "One or more ZFS datasets have unsupported encryption methods. Only datasets with \
             'none' or 'prompt' keylocation are supported. You might need to manually unlock \
             these datasets.",
        );
    }
    let encrypted_roots = datasets
        .datasets
        .values()
        .filter(|dataset| dataset.is_encrypted() && dataset.is_valid_key_root())
        .cloned()
        .collect::<Vec<_>>();
    if !encrypted_roots.is_empty() {
        log::info!(
            "Detected {} encrypted ZFS dataset(s) that require a passphrase to unlock.",
            encrypted_roots.len()
        );
        for dataset in &encrypted_roots {
            if loaded_keys.contains(&dataset.name) {
                log::info!(
                    "Key for ZFS dataset: {} already loaded, skipping prompt.",
                    dataset.name
                );
                continue;
            }
            log::info!("Please enter passphrase for ZFS dataset: {}", dataset.name);
            if load_zfs_key(&dataset.name) {
                log::info!("Successfully loaded key for ZFS dataset: {}", dataset.name);
                loaded_keys.insert(dataset.name.clone());
            } else {
                log::error!(
                    "Failed to load key for ZFS dataset: {}. You will not be able to mount this \
                     dataset and it's children datasets.",
                    dataset.name
                );
            }
        }
    }
    datasets.datasets.values().filter(|dataset| dataset.is_mountable()).cloned().collect()
}
