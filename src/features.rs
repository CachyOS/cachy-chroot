use crate::{depends, utils};

use bitflags::bitflags;
use colored::Colorize;
use which::which;

bitflags! {
  #[derive(Clone, Copy)]
  pub struct Features: u8 {
    const BTRFS = 1 << 0;
    const LUKS = 1 << 1;
    const ZFS  = 1 << 2;
  }
}

pub fn get_enabled_features_from_depends() -> Features {
    let mut enabled_features = Features::empty();
    for dependency in &depends::DEPENDS {
        if which(dependency.command).is_err() {
            if dependency.required {
                utils::print_error_and_exit(&format!(
                    "Required binary not found in path: {}, please install the suggested package: \
                     {}",
                    dependency.command, dependency.package
                ));
            } else {
                log::warn!(
                    "Optional binary not found in path: {}, suggested package: {}, disabled \
                     features: {}",
                    dependency.command.red(),
                    dependency.package.green(),
                    dependency.optional_features_description.yellow()
                );
            }
        } else {
            enabled_features.insert(dependency.features);
        }
    }
    enabled_features
}
