//! Where the UI is shown: the display hardware (Linux DRM/KMS + evdev), or a desktop
//! window for development.

#[cfg(feature = "window")]
pub mod window;

#[cfg(target_os = "linux")]
pub mod kms;
#[cfg(target_os = "linux")]
mod touch;
#[cfg(target_os = "linux")]
mod browser;
