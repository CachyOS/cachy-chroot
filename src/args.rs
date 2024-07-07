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

    /// Disable automatic mounting of block devices based on data from /etc/fstab after root is mounted
    #[arg(long = "no-auto-mount", default_value_t = false)]
    pub no_auto_mount: bool,
}
