//! Wire protocol for AmScope (ToupTek OEM) USB3 microscope cameras.
//!
//! This crate performs no I/O. It encodes the vendor control requests the camera
//! understands and decodes what it sends back. A [`Transport`] supplied by the caller
//! (the `amscope` crate provides one over `nusb`) carries the control transfers. The
//! bulk image stream is decoded by [`frame::FrameAssembler`].
//!
//! Everything here was learned by observing the camera's USB traffic. See
//! `docs/protocol.md` in the repository for the full write-up.

pub mod flash;
pub mod frame;
pub mod isp;
pub mod model;
pub mod request;
pub mod sensor;
pub mod session;

pub use model::{Model, Resolution};
pub use session::{Session, StreamConfig};

/// Errors from a [`Transport`] or from protocol validation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("USB transfer failed: {0}")]
    Transfer(String),
    #[error("camera returned an unexpected response to request {request:#04x}: {detail}")]
    UnexpectedResponse { request: u8, detail: String },
    #[error("value out of range: {0}")]
    OutOfRange(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("corrupt flash data: {0}")]
    Flash(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Carries vendor control transfers to the device (`bmRequestType` `0xc0` / `0x40`,
/// recipient = device).
pub trait Transport {
    /// Vendor IN request. Returns the bytes received, which may be fewer than `length`.
    fn control_in(&mut self, request: u8, value: u16, index: u16, length: u16) -> Result<Vec<u8>>;
    /// Vendor OUT request.
    fn control_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<()>;
}

impl<T: Transport + ?Sized> Transport for &mut T {
    fn control_in(&mut self, request: u8, value: u16, index: u16, length: u16) -> Result<Vec<u8>> {
        (**self).control_in(request, value, index, length)
    }
    fn control_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        (**self).control_out(request, value, index, data)
    }
}
