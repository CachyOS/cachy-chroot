use crate::features::Features;

pub struct Depends {
    pub command: &'static str,
    pub package: &'static str,
    pub required: bool,
    pub optional_features_description: &'static str,
    pub features: Features,
}

pub const DEPENDS: [Depends; 8] = [
    Depends {
        command: "lsblk",
        package: "util-linux",
        required: true,
        optional_features_description: "",
        features: Features::empty(),
    },
    Depends {
        command: "mount",
        package: "util-linux",
        required: true,
        optional_features_description: "",
        features: Features::empty(),
    },
    Depends {
        command: "umount",
        package: "util-linux",
        required: true,
        optional_features_description: "",
        features: Features::empty(),
    },
    Depends {
        command: "arch-chroot",
        package: "arch-install-scripts",
        required: true,
        optional_features_description: "",
        features: Features::empty(),
    },
    Depends {
        command: "btrfs",
        package: "btrfs-progs",
        required: false,
        optional_features_description: "BTRFS Support",
        features: Features::BTRFS,
    },
    Depends {
        command: "cryptsetup",
        package: "cryptsetup",
        required: false,
        optional_features_description: "LUKS Support",
        features: Features::LUKS,
    },
    Depends {
        command: "zfs",
        package: "zfs-utils",
        required: false,
        optional_features_description: "ZFS Support",
        features: Features::ZFS,
    },
    Depends {
        command: "zpool",
        package: "zfs-utils",
        required: false,
        optional_features_description: "ZFS Support",
        features: Features::ZFS,
    },
];
