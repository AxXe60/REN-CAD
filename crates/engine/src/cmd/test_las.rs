//! A synthetic LAS 1.2 file for the command tests.
//!
//! The crate reads LAS and does not write it, and a command that streams a fine band needs a real
//! file to read. The points are `(x, y, z, class)`.

use std::path::Path;

/// Write `points` as a LAS 1.2 file with one centimetre scale.
pub(crate) fn write_test_las(path: &Path, points: &[(f64, f64, f64, u8)]) {
    /// A fixed-size header text field: the text, zero padded.
    fn padded(text: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        let n = text.len().min(out.len());
        out[..n].copy_from_slice(&text[..n]);
        out
    }
    let mut bytes: Vec<u8> = Vec::new();
    bytes.extend_from_slice(b"LASF");
    bytes.extend_from_slice(&0u16.to_le_bytes()); // file source id
    bytes.extend_from_slice(&0u16.to_le_bytes()); // global encoding
    bytes.extend_from_slice(&[0u8; 16]); // project id
    bytes.push(1); // version 1.2
    bytes.push(2);
    bytes.extend_from_slice(&padded(b"CADCraft")); // system identifier
    bytes.extend_from_slice(&padded(b"CADCraft test")); // generating software
    bytes.extend_from_slice(&1u16.to_le_bytes()); // creation day
    bytes.extend_from_slice(&2024u16.to_le_bytes()); // creation year
    bytes.extend_from_slice(&227u16.to_le_bytes()); // header size
    bytes.extend_from_slice(&227u32.to_le_bytes()); // the points start right after the header
    bytes.extend_from_slice(&0u32.to_le_bytes()); // VLRs
    bytes.push(0); // point format 0
    bytes.extend_from_slice(&20u16.to_le_bytes()); // record length
    bytes.extend_from_slice(&(points.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(points.len() as u32).to_le_bytes()); // points of the first return
    bytes.extend_from_slice(&[0u8; 16]); // the other four returns are empty
    let scale = 0.01f64;
    for _ in 0..3 {
        bytes.extend_from_slice(&scale.to_le_bytes()); // scale
    }
    for _ in 0..3 {
        bytes.extend_from_slice(&0f64.to_le_bytes()); // offset
    }
    let (mut min, mut max) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for &(x, y, z, _) in points {
        let p = [x, y, z];
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    for v in [max[0], min[0], max[1], min[1], max[2], min[2]] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    for &(x, y, z, class) in points {
        bytes.extend_from_slice(&((x / scale).round() as i32).to_le_bytes());
        bytes.extend_from_slice(&((y / scale).round() as i32).to_le_bytes());
        bytes.extend_from_slice(&((z / scale).round() as i32).to_le_bytes());
        bytes.extend_from_slice(&100u16.to_le_bytes()); // intensity
        bytes.push(0x11); // one return, the first
        bytes.push(class); // the classification the file carries
        bytes.push(0); // scan angle
        bytes.push(0); // user data
        bytes.extend_from_slice(&0u16.to_le_bytes()); // point source id
    }
    std::fs::write(path, bytes).ok();
}
