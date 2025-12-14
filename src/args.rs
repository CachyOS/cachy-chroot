use clap::Parser;

/// Chroot helper for CachyOS
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Allow running the program without root permissions
    #[arg(long = "skip-root-check", default_value_t = false)]
    pub skip_root_check: bool,

    /// Show .snapshots subvolumes for BTRFS partitions
    #[arg(long = "show-btrfs-dot-snapshots", default_value_t = false)]
    pub show_btrfs_dot_snapshots: bool,

    /// Disable automatic mounting of block devices based on data from /etc/fstab after root is
    /// mounted
    #[arg(long = "no-auto-mount", default_value_t = false)]
    pub no_auto_mount: bool,

    /// Disables arch-chroot systemd mode which spawns a transient systemd instance via systemd-run
    /// inside the chroot. This is useful if you cannot update arch-install-scripts package which
    /// provides this functionality.
    #[arg(long = "no-systemd-chroot", default_value_t = false)]
    pub no_systemd_chroot: bool,
}
