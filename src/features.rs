use bitflags::bitflags;

bitflags! {
  #[derive(Clone, Copy)]
  pub struct Features: u8 {
    const BTRFS = 1 << 0;
    const LUKS = 1 << 1;
  }
}
