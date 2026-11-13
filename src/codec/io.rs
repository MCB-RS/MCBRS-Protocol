//! Low-level read / write functions.
//!
//! Naming scheme: `put_<type>[_<encoding>]` / `get_<type>[_<encoding>]` where
//! the encoding is `le` (little-endian), `be` (big-endian) or `var`
//! (variable-length: LEB128 for unsigned integers, ZigZag + LEB128 for signed
//! ones). The generated code only talks to the wire through these functions.

use super::error::{ProtocolError, Result};
use bytes::{Buf, BufMut};

/// Fails with [`ProtocolError::UnexpectedEof`] unless `needed` bytes are available.
#[inline]
pub fn ensure<B: Buf>(buf: &B, needed: usize) -> Result<()> {
    let remaining = buf.remaining();
    if remaining < needed {
        Err(ProtocolError::UnexpectedEof { needed, remaining })
    } else {
        Ok(())
    }
}

macro_rules! fixed {
    ($($put:ident $get:ident $ty:ty, $size:expr, $bput:ident, $bget:ident;)*) => {$(
        #[inline]
        pub fn $put<B: BufMut>(buf: &mut B, value: $ty) {
            buf.$bput(value);
        }
        #[inline]
        pub fn $get<B: Buf>(buf: &mut B) -> Result<$ty> {
            ensure(buf, $size)?;
            Ok(buf.$bget())
        }
    )*};
}

fixed! {
    put_u8 get_u8 u8, 1, put_u8, get_u8;
    put_i8 get_i8 i8, 1, put_i8, get_i8;
    put_u16_le get_u16_le u16, 2, put_u16_le, get_u16_le;
    put_u16_be get_u16_be u16, 2, put_u16, get_u16;
    put_i16_le get_i16_le i16, 2, put_i16_le, get_i16_le;
    put_i16_be get_i16_be i16, 2, put_i16, get_i16;
    put_u32_le get_u32_le u32, 4, put_u32_le, get_u32_le;
    put_u32_be get_u32_be u32, 4, put_u32, get_u32;
    put_i32_le get_i32_le i32, 4, put_i32_le, get_i32_le;
    put_i32_be get_i32_be i32, 4, put_i32, get_i32;
    put_u64_le get_u64_le u64, 8, put_u64_le, get_u64_le;
    put_u64_be get_u64_be u64, 8, put_u64, get_u64;
    put_i64_le get_i64_le i64, 8, put_i64_le, get_i64_le;
    put_i64_be get_i64_be i64, 8, put_i64, get_i64;
    put_f32_le get_f32_le f32, 4, put_f32_le, get_f32_le;
    put_f32_be get_f32_be f32, 4, put_f32, get_f32;
    put_f64_le get_f64_le f64, 8, put_f64_le, get_f64_le;
    put_f64_be get_f64_be f64, 8, put_f64, get_f64;
}

#[inline]
pub fn put_bool<B: BufMut>(buf: &mut B, value: bool) {
    buf.put_u8(value as u8);
}

#[inline]
pub fn get_bool<B: Buf>(buf: &mut B) -> Result<bool> {
    Ok(get_u8(buf)? != 0)
}

/// Unsigned LEB128, at most 5 bytes.
pub fn put_u32_var<B: BufMut>(buf: &mut B, mut value: u32) {
    while value >= 0x80 {
        buf.put_u8((value as u8) | 0x80);
        value >>= 7;
    }
    buf.put_u8(value as u8);
}

pub fn get_u32_var<B: Buf>(buf: &mut B) -> Result<u32> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = get_u8(buf)?;
        value |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(ProtocolError::VarIntTooLong { max_bytes: 5 })
}

/// Unsigned LEB128, at most 10 bytes.
pub fn put_u64_var<B: BufMut>(buf: &mut B, mut value: u64) {
    while value >= 0x80 {
        buf.put_u8((value as u8) | 0x80);
        value >>= 7;
    }
    buf.put_u8(value as u8);
}

pub fn get_u64_var<B: Buf>(buf: &mut B) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = get_u8(buf)?;
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(ProtocolError::VarIntTooLong { max_bytes: 10 })
}

/// ZigZag + LEB128.
#[inline]
pub fn put_i32_var<B: BufMut>(buf: &mut B, value: i32) {
    put_u32_var(buf, ((value << 1) ^ (value >> 31)) as u32);
}

#[inline]
pub fn get_i32_var<B: Buf>(buf: &mut B) -> Result<i32> {
    let raw = get_u32_var(buf)?;
    Ok(((raw >> 1) as i32) ^ -((raw & 1) as i32))
}

/// ZigZag + LEB128.
#[inline]
pub fn put_i64_var<B: BufMut>(buf: &mut B, value: i64) {
    put_u64_var(buf, ((value << 1) ^ (value >> 63)) as u64);
}

#[inline]
pub fn get_i64_var<B: Buf>(buf: &mut B) -> Result<i64> {
    let raw = get_u64_var(buf)?;
    Ok(((raw >> 1) as i64) ^ -((raw & 1) as i64))
}

macro_rules! narrow_var {
    ($($put:ident $get:ident $ty:ty => $wide:ty, $wput:ident, $wget:ident;)*) => {$(
        /// Narrow integer transported as a 32-bit varint.
        #[inline]
        pub fn $put<B: BufMut>(buf: &mut B, value: $ty) {
            $wput(buf, value as $wide);
        }
        #[inline]
        pub fn $get<B: Buf>(buf: &mut B) -> Result<$ty> {
            let wide = $wget(buf)?;
            <$ty>::try_from(wide).map_err(|_| ProtocolError::IntegerOutOfRange {
                type_name: stringify!($ty),
                value: wide as i128,
            })
        }
    )*};
}

narrow_var! {
    put_u8_var get_u8_var u8 => u32, put_u32_var, get_u32_var;
    put_u16_var get_u16_var u16 => u32, put_u32_var, get_u32_var;
    put_i8_var get_i8_var i8 => i32, put_i32_var, get_i32_var;
    put_i16_var get_i16_var i16 => i32, put_i32_var, get_i32_var;
}

/// Collection length prefixed as an unsigned 32-bit varint (the default).
pub fn put_len_var<B: BufMut>(buf: &mut B, len: usize) -> Result<()> {
    let len = u32::try_from(len).map_err(|_| ProtocolError::LengthOutOfRange { length: len as u64 })?;
    put_u32_var(buf, len);
    Ok(())
}

pub fn get_len_var<B: Buf>(buf: &mut B) -> Result<usize> {
    Ok(get_u32_var(buf)? as usize)
}

/// Collection length prefixed as a fixed little-endian `u32`
/// (`"No size compression"` in Mojang's schemas).
pub fn put_len_u32_le<B: BufMut>(buf: &mut B, len: usize) -> Result<()> {
    let len = u32::try_from(len).map_err(|_| ProtocolError::LengthOutOfRange { length: len as u64 })?;
    buf.put_u32_le(len);
    Ok(())
}

pub fn get_len_u32_le<B: Buf>(buf: &mut B) -> Result<usize> {
    Ok(get_u32_le(buf)? as usize)
}

/// Checks that a collection without length prefix holds exactly `expected` elements.
pub fn check_len(actual: usize, expected: usize) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(ProtocolError::LengthOutOfRange { length: actual as u64 })
    }
}

/// Raw bytes without any prefix.
pub fn get_bytes<B: Buf>(buf: &mut B, len: usize) -> Result<Vec<u8>> {
    ensure(buf, len)?;
    let mut out = vec![0u8; len];
    buf.copy_to_slice(&mut out);
    Ok(out)
}

/// UTF-8 string prefixed by its byte length as an unsigned 32-bit varint.
pub fn put_string<B: BufMut>(buf: &mut B, value: &str) -> Result<()> {
    put_len_var(buf, value.len())?;
    buf.put_slice(value.as_bytes());
    Ok(())
}

pub fn get_string<B: Buf>(buf: &mut B) -> Result<String> {
    let len = get_len_var(buf)?;
    String::from_utf8(get_bytes(buf, len)?).map_err(|_| ProtocolError::InvalidUtf8)
}

/// Upper bound, in bytes, of what a length prefix may reserve up front.
const MAX_PREALLOCATION: usize = 64 * 1024;

/// Largest collection of elements that occupy no byte at all on the wire.
const MAX_EMPTY_ELEMENTS: usize = 1 << 16;

/// Reads `len` elements with `read`.
///
/// A length prefix comes from the peer and cannot be trusted: the
/// pre-allocation is bounded (the vector then grows only as elements are
/// actually decoded), and a huge count of elements that consume no input is
/// rejected instead of spinning.
pub fn get_vec<B: Buf, T>(buf: &mut B, len: usize, mut read: impl FnMut(&mut B) -> Result<T>) -> Result<Vec<T>> {
    let element_size = std::mem::size_of::<T>().max(1);
    let mut out = Vec::with_capacity(len.min(buf.remaining()).min(MAX_PREALLOCATION / element_size));
    for _ in 0..len {
        let before = buf.remaining();
        out.push(read(buf)?);
        if len > MAX_EMPTY_ELEMENTS && buf.remaining() == before {
            return Err(ProtocolError::LengthOutOfRange { length: len as u64 });
        }
    }
    Ok(out)
}

/// Reads exactly `N` elements with `read` (fixed-size arrays have no prefix).
pub fn get_array<B: Buf, T, const N: usize>(buf: &mut B, read: impl FnMut(&mut B) -> Result<T>) -> Result<[T; N]> {
    let items = get_vec(buf, N, read)?;
    match <[T; N]>::try_from(items) {
        Ok(array) => Ok(array),
        Err(items) => Err(ProtocolError::LengthOutOfRange { length: items.len() as u64 }),
    }
}
