//! LZAV format-3 解壓（EU5 地形貼花的每個小塊都是一段 LZAV 串流）。
//!
//! 移植自原專案 `old/native/src/lzav.rs`（MPL-2.0）；原專案依 LZAV 5.18 by Aleksey Vaneev
//! (MIT, <https://github.com/avaneev/lzav>, commit 539f189193e1e81cddcccf554c8fe17dc107e876);
//! no LZAV code is copied.

const FORMAT: u8 = 3;
const SHORT_LENGTH: usize = 16;
const VARINT_LIMIT: u32 = 28;
const OFFSET_MASKS: [u64; 4] = [0, 0x3FF, 0x7FFF, 0x001F_FFFF];
const CARRY_SHIFTS: [u32; 4] = [0, 0, 3, 5];

/// Why a stream was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Fault {
    NotLzav,
    Truncated,
    BadReference,
    Size { decoded: usize, expected: usize },
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLzav => f.write_str("not an LZAV format-3 stream"),
            Self::Truncated => f.write_str("truncated LZAV stream"),
            Self::BadReference => f.write_str("reference outside decoded data"),
            Self::Size { decoded, expected } => {
                write!(f, "decoded {decoded} bytes, expected {expected}")
            }
        }
    }
}

fn varint(data: &[u8], mut index: usize) -> Result<(usize, usize), Fault> {
    let mut value: usize = 0;
    let mut shift: u32 = 0;
    loop {
        let byte = *data.get(index).ok_or(Fault::Truncated)?;
        index += 1;
        value |= usize::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 || shift >= VARINT_LIMIT - 7 {
            return Ok((value, index));
        }
        shift += 7;
    }
}

/// Decode a format-3 stream that expands to exactly `size` bytes.
pub fn decode(data: &[u8], size: usize) -> Result<Vec<u8>, Fault> {
    if data.len() < 2 || data[0] >> 4 != FORMAT || data[0] & 15 == 0 {
        return Err(Fault::NotLzav);
    }
    let extra = usize::from(data[0] & 15) - 1;
    let mut out: Vec<u8> = Vec::with_capacity(size);
    let mut index = 1;
    let mut carry: u64 = 0;
    let mut shift: u32 = 0;
    while index < data.len() {
        let header = data[index];
        let kind = usize::from((header >> 4) & 3);
        index += 1;
        if kind == 0 {
            let mut count = usize::from(header & 15);
            if count == 0 {
                let (value, next) = varint(data, index)?;
                count = value + SHORT_LENGTH;
                index = next;
            }
            carry |= u64::from(header >> 6) << shift;
            shift += 2;
            let end = (index + count).min(data.len());
            out.extend_from_slice(&data[index..end]);
            index += count;
            continue;
        }
        let mut word: u64 = 0;
        for (offset, byte) in data.iter().skip(index).take(4).enumerate() {
            word |= u64::from(*byte) << (8 * offset);
        }
        let distance =
            ((((word << 2) | u64::from(header >> 6)) & OFFSET_MASKS[kind]) << shift) | carry;
        let new_shift = CARRY_SHIFTS[kind];
        let last = *data.get(index + kind - 1).ok_or(Fault::Truncated)?;
        carry = (u64::from(last) << new_shift) >> 8;
        shift = new_shift;
        index += kind;
        let count = usize::from(header & 15);
        let length = if count != 0 {
            extra + count
        } else {
            let (value, next) = varint(data, index)?;
            index = next;
            value + extra + SHORT_LENGTH
        };
        let distance = usize::try_from(distance).map_err(|_| Fault::BadReference)?;
        if distance == 0 || distance > out.len() {
            return Err(Fault::BadReference);
        }
        let start = out.len() - distance;
        if distance >= length {
            out.extend_from_within(start..start + length);
        } else {
            for step in 0..length {
                let byte = out[start + step];
                out.push(byte);
            }
        }
    }
    if out.len() != size {
        return Err(Fault::Size {
            decoded: out.len(),
            expected: size,
        });
    }
    Ok(out)
}

const MIN_REFERENCE: usize = 5;
const LITERALS_AT_END: usize = 9;
const SHORT_REFERENCE: usize = 15;
const RUN_DISTANCE: usize = 8;
const MIN_RUN: usize = 32;
pub 
fn push_length(out: &mut Vec<u8>, mut value: usize) {
    while value > 0x7F {
        out.push(0x80 | (value & 0x7F) as u8);
        value >>= 7;
    }
    out.push(value as u8);
}

fn push_literals(out: &mut Vec<u8>, chunk: &[u8]) {
    if chunk.len() < SHORT_LENGTH {
        out.push(chunk.len() as u8);
    } else {
        out.push(0);
        push_length(out, chunk.len() - SHORT_LENGTH);
    }
    out.extend_from_slice(chunk);
}

/// Spans `[start, end)` where every byte repeats the byte eight positions earlier and the
/// run is at least `MIN_RUN` long.
fn runs(raw: &[u8]) -> Vec<(usize, usize)> {
    if raw.len() <= RUN_DISTANCE {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let mut start: Option<usize> = None;
    for at in 0..=raw.len() - RUN_DISTANCE {
        let same = at < raw.len() - RUN_DISTANCE && raw[at + RUN_DISTANCE] == raw[at];
        match (same, start) {
            (true, None) => start = Some(at),
            (false, Some(first)) => {
                if at - first >= MIN_RUN {
                    spans.push((first + RUN_DISTANCE, at + RUN_DISTANCE));
                }
                start = None;
            }
            _ => {}
        }
    }
    spans
}

/// 壓縮：重複段落（每個位元組等於 8 個之前的）用往前 8 的參照，其餘直接存（移植自原專案 `old/native/src/tiles.rs`）。
pub fn encode(raw: &[u8]) -> Vec<u8> {
    let limit = raw.len() as i64 - LITERALS_AT_END as i64;
    let mut out = vec![FORMAT << 4 | MIN_REFERENCE as u8];
    let mut position = 0_usize;
    for (run_start, run_end) in runs(raw) {
        let start = run_start.max(position);
        let end = (run_end as i64).min(limit);
        if end - (start as i64) < MIN_REFERENCE as i64 + 1 {
            continue;
        }
        let end = end as usize;
        let mut shift = 0;
        if start > position {
            push_literals(&mut out, &raw[position..start]);
            shift = 2;
        }
        let field = (RUN_DISTANCE >> shift) as u8;
        let length = end - start;
        let extra = MIN_REFERENCE - 1;
        if length - extra <= SHORT_REFERENCE {
            out.push((field & 3) << 6 | 0x10 | (length - extra) as u8);
            out.push(field >> 2);
        } else {
            out.push((field & 3) << 6 | 0x10);
            out.push(field >> 2);
            push_length(&mut out, length - extra - SHORT_LENGTH);
        }
        position = end;
    }
    let tail = &raw[position..];
    push_literals(&mut out, tail);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_literal_block() {
        // prefix 3<<4|5, a literal block of 5 bytes (kind 0, count 5)
        let stream = [0x35, 0x05, b'h', b'e', b'l', b'l', b'o'];
        assert_eq!(decode(&stream, 5).unwrap(), b"hello");
        assert_eq!(
            decode(&stream, 6),
            Err(Fault::Size {
                decoded: 5,
                expected: 6
            })
        );
    }

    #[test]
    fn encoded_streams_decode_to_the_input() {
        let mut raw: Vec<u8> = (0..300).map(|v| (v * 7 % 251) as u8).collect();
        raw.extend(std::iter::repeat_n(9_u8, 400));
        raw.extend((0..50).map(|v| v as u8));
        raw.extend([1, 2, 3, 4, 5, 6, 7, 8].repeat(40));
        for cut in [1, 8, 15, 16, 17, 40, raw.len()] {
            let part = &raw[..cut];
            assert_eq!(decode(&encode(part), part.len()).unwrap(), part, "length {cut}");
        }
        assert!(encode(&raw).len() < raw.len() / 2, "runs are compressed");
    }

    #[test]
    fn refuses_other_formats_and_bad_references() {
        assert_eq!(decode(&[0x25, 0x00], 0), Err(Fault::NotLzav));
        // a kind-1 reference with distance 8 before any output
        let stream = [0x35, 0x11 | 0x20, 0x00];
        assert!(decode(&stream, 10).is_err());
    }
}
