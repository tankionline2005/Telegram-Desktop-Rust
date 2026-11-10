//! Telegram API types generated from `schema/api.tl` (and the MTProto service
//! layer from `schema/mtproto.tl`), plus the (de)serialization runtime.
//!
//! * [`Serializable`] writes boxed values (constructor id + fields) into a `Vec<u8>`.
//! * [`Deserializable`] reads from a [`Reader`] over a [`Bytes`] buffer; `bytes` fields
//!   are zero-copy slices of that buffer.
//! * [`RemoteCall`] is implemented by every RPC method and names its return type.

pub use bytes::Bytes;

mod runtime;
pub use runtime::*;

include!(concat!(env!("OUT_DIR"), "/api.rs"));

/// TL layer of `schema/api.tl`; sent in `invokeWithLayer`.
pub const LAYER: i32 = include!(concat!(env!("OUT_DIR"), "/layer.rs"));

/// MTProto service types (`schema/mtproto.tl`). Definitions with the polymorphic
/// `Object` type (`rpc_result`, `msg_container`, `gzip_packed`) are handled by hand
/// in the transport layer.
pub mod mtproto {
    include!(concat!(env!("OUT_DIR"), "/mtproto.rs"));
}

#[cfg(test)]
mod tests;
