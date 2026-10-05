//! Decoder traits implemented by the codec crates and driven by the player.
use crate::{AudioBuffer, Packet, Result, StreamInfo, VideoFrame};
use alloc::boxed::Box;

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

/// Creates decoders for streams. Implemented by the app or host that links the codec crates, so the
/// player core stays free of codec dependencies.
pub trait CodecFactory {
    /// A decoder for an audio stream, or `Unsupported`.
    fn audio(&self, info: &StreamInfo) -> Result<Box<dyn AudioDecoder>>;
    /// A decoder for a video stream, or `Unsupported`.
    fn video(&self, info: &StreamInfo) -> Result<Box<dyn VideoDecoder>>;
}
