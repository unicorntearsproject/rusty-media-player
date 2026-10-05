//! Decoder traits implemented by the codec crates and driven by the player.
use crate::{AudioBuffer, Packet, Result, VideoFrame};

/// A video decoder. Send packets in decode order, receive frames in presentation order.
pub trait VideoDecoder {
    /// Feed one compressed packet.
    fn send_packet(&mut self, packet: &Packet) -> Result<()>;
    /// Take the next decoded frame, or `None` if more input is needed.
    fn receive_frame(&mut self) -> Result<Option<VideoFrame>>;
    /// Drop all internal state (seek, stream change).
    fn flush(&mut self);
}

/// An audio decoder.
pub trait AudioDecoder {
    /// Feed one compressed packet.
    fn send_packet(&mut self, packet: &Packet) -> Result<()>;
    /// Take the next decoded buffer, or `None` if more input is needed.
    fn receive_buffer(&mut self) -> Result<Option<AudioBuffer>>;
    /// Drop all internal state.
    fn flush(&mut self);
}
