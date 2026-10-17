# loupecam-protocol

The wire protocol for AmScope / ToupTek USB3 microscope cameras, with no I/O.

It covers:

- the vendor's scrambled control-request encoding
- the session handshake
- sensor and FPGA register programming
- the camera models
- frame assembly from bulk-transfer data

It talks to the camera through a small `Transport` trait, so it can be tested
against recorded traffic. The tests check that it produces the same control sequences
as the vendor SDK.

Most users want [`loupecam`](https://crates.io/crates/loupecam), which supplies the
USB transport. The protocol is documented in
[docs/protocol.md](https://github.com/cinderblock/loupecam/blob/master/docs/protocol.md).

## License

MIT OR Apache-2.0.
