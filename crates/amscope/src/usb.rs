//! [`Transport`] over an `nusb` interface.

use amscope_protocol::{Error, Result, Transport};
use nusb::MaybeFuture;
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient};
use std::time::Duration;

pub(crate) struct UsbTransport {
    interface: nusb::Interface,
    timeout: Duration,
}

impl UsbTransport {
    pub fn new(interface: nusb::Interface, timeout: Duration) -> Self {
        UsbTransport { interface, timeout }
    }
}

impl Transport for UsbTransport {
    fn control_in(&mut self, request: u8, value: u16, index: u16, length: u16) -> Result<Vec<u8>> {
        self.interface
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request,
                    value,
                    index,
                    length,
                },
                self.timeout,
            )
            .wait()
            .map_err(|e| Error::Transfer(format!("IN {request:#04x} wValue={value:#06x} wIndex={index:#06x}: {e}")))
    }

    fn control_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        self.interface
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request,
                    value,
                    index,
                    data,
                },
                self.timeout,
            )
            .wait()
            .map_err(|e| Error::Transfer(format!("OUT {request:#04x} wValue={value:#06x} wIndex={index:#06x}: {e}")))
    }
}
