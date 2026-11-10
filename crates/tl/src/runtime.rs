use std::fmt;

use bytes::Bytes;

/// Id of the boxed `Vector t` constructor.
pub const VECTOR_ID: u32 = 0x1cb5_c415;
const BOOL_TRUE: u32 = 0x9972_75b5;
const BOOL_FALSE: u32 = 0xbc79_9737;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeserializeError {
    /// The buffer ended before the value was complete.
    UnexpectedEof,
    /// A constructor id that does not belong to the expected type.
    UnexpectedConstructor { id: u32 },
}

impl fmt::Display for DeserializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => f.write_str("unexpected end of buffer"),
            Self::UnexpectedConstructor { id } => {
                let name = crate::name_for_id(*id)
                    .or_else(|| crate::mtproto::name_for_id(*id))
                    .unwrap_or("unknown");
                write!(f, "unexpected constructor {id:#010x} ({name})")
            }
        }
    }
}

impl std::error::Error for DeserializeError {}

pub type Result<T> = std::result::Result<T, DeserializeError>;

/// Cursor over a received buffer. Reading `bytes` returns slices of the same allocation.
pub struct Reader<'a> {
    src: &'a Bytes,
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(src: &'a Bytes) -> Self {
        Self { src, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.src.len() - self.pos
    }

    /// Everything not read yet, without copying.
    pub fn rest(&self) -> Bytes {
        self.src.slice(self.pos..)
    }

    #[inline]
    pub fn read_slice(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(DeserializeError::UnexpectedEof)?;
        let slice = self
            .src
            .get(self.pos..end)
            .ok_or(DeserializeError::UnexpectedEof)?;
        self.pos = end;
        Ok(slice)
    }

    #[inline]
    pub fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let s = self.read_slice(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }

    #[inline]
    pub fn read_u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.read_array()?))
    }

    #[inline]
    pub fn peek_u32(&self) -> Result<u32> {
        let s = self
            .src
            .get(self.pos..self.pos + 4)
            .ok_or(DeserializeError::UnexpectedEof)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    /// Reads TL `bytes`/`string` framing and returns the payload range.
    fn read_tl_range(&mut self) -> Result<std::ops::Range<usize>> {
        let first = self.read_slice(1)?[0];
        let (len, header) = if first < 254 {
            (first as usize, 1)
        } else {
            let b = self.read_slice(3)?;
            (
                b[0] as usize | (b[1] as usize) << 8 | (b[2] as usize) << 16,
                4,
            )
        };
        let start = self.pos;
        self.read_slice(len)?;
        let padding = (4 - (header + len) % 4) % 4;
        self.read_slice(padding)?;
        Ok(start..start + len)
    }

    /// TL `bytes` as a zero-copy slice.
    pub fn read_bytes(&mut self) -> Result<Bytes> {
        let range = self.read_tl_range()?;
        Ok(self.src.slice(range))
    }

    /// TL `bytes` borrowed from the buffer.
    pub fn read_bytes_ref(&mut self) -> Result<&'a [u8]> {
        let range = self.read_tl_range()?;
        Ok(&self.src[range])
    }
}

/// Writes TL `bytes`/`string` framing.
pub fn write_tl_bytes(out: &mut Vec<u8>, data: &[u8]) {
    let len = data.len();
    let header = if len < 254 {
        out.push(len as u8);
        1
    } else {
        assert!(len < 1 << 24, "TL bytes longer than 16 MiB");
        out.extend_from_slice(&[254, len as u8, (len >> 8) as u8, (len >> 16) as u8]);
        4
    };
    out.extend_from_slice(data);
    let padding = (4 - (header + len) % 4) % 4;
    out.extend_from_slice(&[0u8; 3][..padding]);
}

pub trait Serializable {
    fn serialize(&self, out: &mut Vec<u8>);

    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.serialize(&mut out);
        out
    }
}

pub trait Deserializable: Sized {
    fn deserialize(r: &mut Reader<'_>) -> Result<Self>;

    fn from_bytes(data: &Bytes) -> Result<Self> {
        Self::deserialize(&mut Reader::new(data))
    }
}

/// Constructors and methods with a fixed id.
pub trait Identifiable {
    const CONSTRUCTOR_ID: u32;
}

/// Serialization without the constructor id (used inside enums and bare vectors).
pub trait Bare: Sized {
    fn serialize_bare(&self, out: &mut Vec<u8>);
    fn deserialize_bare(r: &mut Reader<'_>) -> Result<Self>;
}

/// An RPC method.
pub trait RemoteCall: Serializable {
    type Return: Deserializable;
}

macro_rules! primitive {
    ($t:ty) => {
        impl Serializable for $t {
            #[inline]
            fn serialize(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
        }
        impl Deserializable for $t {
            #[inline]
            fn deserialize(r: &mut Reader<'_>) -> Result<Self> {
                Ok(<$t>::from_le_bytes(r.read_array()?))
            }
        }
    };
}
primitive!(i32);
primitive!(u32);
primitive!(i64);
primitive!(f64);

impl<const N: usize> Serializable for [u8; N] {
    fn serialize(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(self);
    }
}
impl<const N: usize> Deserializable for [u8; N] {
    fn deserialize(r: &mut Reader<'_>) -> Result<Self> {
        r.read_array()
    }
}

impl Serializable for bool {
    fn serialize(&self, out: &mut Vec<u8>) {
        let id = if *self { BOOL_TRUE } else { BOOL_FALSE };
        out.extend_from_slice(&id.to_le_bytes());
    }
}
impl Deserializable for bool {
    fn deserialize(r: &mut Reader<'_>) -> Result<Self> {
        match r.read_u32()? {
            BOOL_TRUE => Ok(true),
            BOOL_FALSE => Ok(false),
            id => Err(DeserializeError::UnexpectedConstructor { id }),
        }
    }
}

impl Serializable for String {
    fn serialize(&self, out: &mut Vec<u8>) {
        write_tl_bytes(out, self.as_bytes());
    }
}
impl Serializable for str {
    fn serialize(&self, out: &mut Vec<u8>) {
        write_tl_bytes(out, self.as_bytes());
    }
}
impl Deserializable for String {
    fn deserialize(r: &mut Reader<'_>) -> Result<Self> {
        // Telegram occasionally sends invalid UTF-8; keep what we can, like tdesktop.
        Ok(String::from_utf8_lossy(r.read_bytes_ref()?).into_owned())
    }
}

impl Serializable for Bytes {
    fn serialize(&self, out: &mut Vec<u8>) {
        write_tl_bytes(out, self);
    }
}
impl Deserializable for Bytes {
    fn deserialize(r: &mut Reader<'_>) -> Result<Self> {
        r.read_bytes()
    }
}

impl<T: Serializable> Serializable for Vec<T> {
    fn serialize(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&VECTOR_ID.to_le_bytes());
        out.extend_from_slice(&(self.len() as u32).to_le_bytes());
        for x in self {
            x.serialize(out);
        }
    }
}
impl<T: Deserializable> Deserializable for Vec<T> {
    fn deserialize(r: &mut Reader<'_>) -> Result<Self> {
        let id = r.read_u32()?;
        if id != VECTOR_ID {
            return Err(DeserializeError::UnexpectedConstructor { id });
        }
        read_items(r)
    }
}

fn read_items<T: Deserializable>(r: &mut Reader<'_>) -> Result<Vec<T>> {
    let len = r.read_u32()? as usize;
    // Every item takes at least 4 bytes: don't trust a huge length from the wire.
    if len > r.remaining() / 4 {
        return Err(DeserializeError::UnexpectedEof);
    }
    let mut items = Vec::with_capacity(len);
    for _ in 0..len {
        items.push(T::deserialize(r)?);
    }
    Ok(items)
}

/// Bare `vector<T>` (no constructor id), used by a few MTProto types.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RawVec<T>(pub Vec<T>);

impl<T: Serializable> Serializable for RawVec<T> {
    fn serialize(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.0.len() as u32).to_le_bytes());
        for x in &self.0 {
            x.serialize(out);
        }
    }
}
impl<T: Deserializable> Deserializable for RawVec<T> {
    fn deserialize(r: &mut Reader<'_>) -> Result<Self> {
        read_items(r).map(RawVec)
    }
}
