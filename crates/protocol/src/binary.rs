//! Canonical bounded binary transport shared by Image, document and map chunks.
use sha2::{Digest, Sha256};

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
/// Hash signed ROS cell bytes without allocating a second complete map.
pub fn signed_sha256(cells: &[i8]) -> String {
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 4096];
    for part in cells.chunks(buffer.len()) {
        for (target, source) in buffer.iter_mut().zip(part) {
            *target = *source as u8;
        }
        hasher.update(&buffer[..part.len()]);
    }
    format!("{:x}", hasher.finalize())
}
pub fn valid_sha256(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
/// Exact contiguous chunk layout, validated before decoding or allocation.
pub fn chunk_length(
    total: usize,
    index: usize,
    count: usize,
    max_total: usize,
    chunk_bytes: usize,
) -> Result<usize, String> {
    if chunk_bytes == 0
        || total == 0
        || total > max_total
        || count != total.div_ceil(chunk_bytes)
        || index >= count
    {
        return Err("Invalid bounded chunk layout".into());
    }
    let offset = index
        .checked_mul(chunk_bytes)
        .ok_or("Chunk offset overflow")?;
    total
        .checked_sub(offset)
        .map(|remaining| remaining.min(chunk_bytes))
        .ok_or_else(|| "Chunk offset exceeds data".into())
}
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
pub fn encode(raw: &[u8]) -> String {
    let mut output = String::with_capacity(raw.len().div_ceil(3) * 4);
    for bytes in raw.chunks(3) {
        let a = bytes[0];
        let b = bytes.get(1).copied().unwrap_or(0);
        let c = bytes.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[(a >> 2) as usize] as char);
        output.push(ALPHABET[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        output.push(if bytes.len() > 1 {
            ALPHABET[(((b & 15) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if bytes.len() > 2 {
            ALPHABET[(c & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}
fn digit(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}
pub fn validate_exact(text: &str, expected: usize, max_raw: usize) -> Result<(), String> {
    let encoded_length = expected
        .div_ceil(3)
        .checked_mul(4)
        .ok_or("base64 encoded length overflow")?;
    if expected > max_raw
        || text.len() > max_raw.div_ceil(3).saturating_mul(4)
        || text.len() != encoded_length
    {
        return Err("base64 length contradicts exact raw length".into());
    }
    let last = text.len().saturating_sub(4);
    for (index, chunk) in text.as_bytes().chunks_exact(4).enumerate() {
        let a = digit(chunk[0]);
        let b = digit(chunk[1]);
        if a.is_none() || b.is_none() {
            return Err("Invalid canonical base64".into());
        }
        let tail = index * 4 == last;
        let padding = if tail { (3 - expected % 3) % 3 } else { 0 };
        match padding {
            2 if chunk[2] == b'=' && chunk[3] == b'=' && b.unwrap() & 15 == 0 => (),
            1 if digit(chunk[2]).is_some_and(|c| c & 3 == 0) && chunk[3] == b'=' => (),
            0 if digit(chunk[2]).is_some() && digit(chunk[3]).is_some() => (),
            _ => return Err("Invalid canonical base64 padding or trailing bits".into()),
        }
    }
    Ok(())
}

/// Validate before allocation and decode exactly the declared number of bytes.
pub fn decode_exact(text: &str, expected: usize, max_raw: usize) -> Result<Vec<u8>, String> {
    validate_exact(text, expected, max_raw)?;
    let mut output = Vec::with_capacity(expected);
    for chunk in text.as_bytes().chunks_exact(4) {
        let a = digit(chunk[0]).expect("validated base64");
        let b = digit(chunk[1]).expect("validated base64");
        output.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            let c = digit(chunk[2]).expect("validated base64");
            output.push((b << 4) | (c >> 2));
            if chunk[3] != b'=' {
                output.push((c << 6) | digit(chunk[3]).expect("validated base64"));
            }
        }
    }
    debug_assert_eq!(output.len(), expected);
    Ok(output)
}
