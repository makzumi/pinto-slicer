//! Creality CXDLP v3 and v4 writers.
//!
//! Layouts follow UVtools' CrealityCXDLPFile and CrealityCXDLPv4File. In
//! particular, v4 is not a small "CXDLP" header with byte-pair RLE: it starts
//! with `CXSW3DV2`, carries mixed-endian printer/header records, a 40-byte
//! layer table plus 44-byte per-layer records, and a final big-endian CRC32.
//! HALOT-X1 layers use its row-bounded 3-bit pixel-pair RLE.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::config::{PrintSettings, PrinterProfile};
use crate::formats::{self, Progress};
use crate::slicer::SliceResult;

const MAGIC: &[u8; 9] = b"CXSW3DV2\0";
const PRINT_PARAMETERS_SIZE: usize = 68;
const SLICER_INFO_SIZE: usize = 76;
const LAYER_DEF_SIZE: usize = 40;
const LAYER_DEF_EX_SIZE: usize = 44;

fn be16(w: &mut impl Write, v: u16) -> std::io::Result<()> {
    w.write_all(&v.to_be_bytes())
}
fn be32(w: &mut impl Write, v: u32) -> std::io::Result<()> {
    w.write_all(&v.to_be_bytes())
}
fn le16(w: &mut impl Write, v: u16) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn le32(w: &mut impl Write, v: u32) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn lef(w: &mut impl Write, v: f32) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn cstring_be(w: &mut impl Write, value: &str) -> std::io::Result<()> {
    be32(w, value.len() as u32 + 1)?;
    w.write_all(value.as_bytes())?;
    w.write_all(&[0])
}

/// Creality uses the CRC32 polynomial with an initial register of zero and no
/// final XOR (unlike the conventional crc32fast defaults).
fn crc32(bytes: &[u8], mut state: u32) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            let mut value = i as u32;
            for _ in 0..8 {
                value = if value & 1 != 0 {
                    (value >> 1) ^ 0xEDB8_8320
                } else {
                    value >> 1
                };
            }
            *entry = value;
        }
        table
    });
    for byte in bytes {
        let index = ((state as u8) ^ *byte) as usize;
        state = table[index] ^ (state >> 8);
    }
    state
}

fn append_checksum(file: &mut std::fs::File) -> Result<(), String> {
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut state = 0;
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        state = crc32(&buf[..n], state);
    }
    file.write_all(&state.to_be_bytes())
        .map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())
}

/// The seven-bit gray RLE shared by classic CXDLPv4 and CTB.
pub(crate) fn rle7(mask: &[u8]) -> Vec<u8> {
    fn add(out: &mut Vec<u8>, mut count: usize, gray: u8) {
        while count > 0 {
            let run = count.min(0x0FFF_FFFF);
            out.push((gray >> 1) | if run > 1 { 0x80 } else { 0 });
            if run > 1 {
                if run <= 0x7f {
                    out.push(run as u8);
                } else if run <= 0x3fff {
                    out.push(((run >> 8) as u8) | 0x80);
                    out.push(run as u8);
                } else if run <= 0x1f_ffff {
                    out.push(((run >> 16) as u8) | 0xc0);
                    out.push((run >> 8) as u8);
                    out.push(run as u8);
                } else {
                    out.push(((run >> 24) as u8) | 0xe0);
                    out.push((run >> 16) as u8);
                    out.push((run >> 8) as u8);
                    out.push(run as u8);
                }
            }
            count -= run;
        }
    }
    let mut out = Vec::with_capacity(mask.len() / 32 + 16);
    let mut at = 0;
    while at < mask.len() {
        let gray = mask[at] >> 1;
        let mut end = at + 1;
        while end < mask.len() && mask[end] >> 1 == gray {
            end += 1;
        }
        add(&mut out, end - at, gray << 1);
        at = end;
    }
    out
}

/// HALOT-X1: two 3-bit pixels per unit, high bit set, then an optional
/// big-endian base-128 repeat count. Runs never cross a row.
pub(crate) fn pixel_pair_rle(mask: &[u8], width: usize) -> Result<Vec<u8>, String> {
    if width == 0 || !width.is_multiple_of(2) || !mask.len().is_multiple_of(width) {
        return Err("HALOT-X1 pixel-pair encoding needs an even image width".into());
    }
    let units_per_row = width / 2;
    let unit =
        |row: &[u8], at: usize| -> u8 { 0x80 | (row[at * 2] >> 5) | ((row[at * 2 + 1] >> 5) << 3) };
    let mut out = Vec::with_capacity(mask.len() / 64 + 16);
    for row in mask.chunks_exact(width) {
        let mut at = 0;
        while at < units_per_row {
            let value = unit(row, at);
            let mut end = at + 1;
            while end < units_per_row && unit(row, end) == value {
                end += 1;
            }
            out.push(value);
            let repeat = end - at - 1;
            if repeat > 0 {
                let groups = (usize::BITS - repeat.leading_zeros()).div_ceil(7).max(1);
                for group in (0..groups).rev() {
                    out.push(((repeat >> (group * 7)) & 0x7f) as u8);
                }
            }
            at = end;
        }
    }
    Ok(out)
}

#[cfg(test)]
fn decode_pixel_pair(rle: &[u8], width: usize, height: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(width * height);
    let mut at = 0;
    let mut unit_index = 0usize;
    let units_per_row = width / 2;
    while at < rle.len() {
        let code = rle[at];
        at += 1;
        if code & 0x80 == 0 || code & 0x40 != 0 {
            return Err("bad pixel-pair unit".into());
        }
        let remaining = units_per_row - unit_index % units_per_row;
        let mut repeat = 0usize;
        while at < rle.len() && rle[at] & 0x80 == 0 {
            repeat = (repeat << 7) | rle[at] as usize;
            at += 1;
            if repeat >= remaining {
                return Err("run crossed a row".into());
            }
        }
        let left = code & 7;
        let right = (code >> 3) & 7;
        let expand = |g: u8| (g << 5) | (g << 2) | (g >> 1);
        for _ in 0..repeat + 1 {
            out.push(expand(left));
            out.push(expand(right));
        }
        unit_index += repeat + 1;
    }
    if out.len() != width * height {
        return Err("incomplete pixel-pair image".into());
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn write_v4_header(
    file: &mut std::fs::File,
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    res: (usize, usize),
    layers_offset: u32,
    params_offset: u32,
    slicer_offset: u32,
) -> std::io::Result<()> {
    be32(file, 9)?;
    file.write_all(MAGIC)?;
    be16(file, 4)?;
    cstring_be(file, &printer.machine_name)?;
    le16(file, res.0 as u16)?;
    le16(file, res.1 as u16)?;
    for v in [
        printer.build[0],
        printer.build[1],
        printer.build[2],
        slice.height,
        slice.layer_height,
    ] {
        lef(file, v)?;
    }
    le32(file, settings.bottom_layers)?;
    le32(file, 0)?; // small preview
    le32(file, layers_offset)?;
    le32(file, slice.layers.len() as u32)?;
    le32(file, 0)?; // large preview
    le32(
        file,
        (settings.estimated_minutes(slice.layers.len()) * 60.0).round() as u32,
    )?;
    le32(file, printer.mirror[0] as u32)?;
    le32(file, params_offset)?;
    le32(file, PRINT_PARAMETERS_SIZE as u32)?;
    le32(file, 1)?; // antialias level
    le16(file, 255)?;
    le16(file, 255)?;
    le32(file, 0)?; // encryption disabled
    le32(file, slicer_offset)?;
    le32(file, SLICER_INFO_SIZE as u32)
}

fn write_v4_params(file: &mut std::fs::File, settings: &PrintSettings) -> std::io::Result<()> {
    for v in [
        8.0,
        300.0,
        5.0,
        300.0,
        300.0,
        0.0,
        0.0,
        0.0,
        settings.layer_move_s,
        settings.layer_move_s,
    ] {
        lef(file, v)?;
    }
    le32(file, settings.bottom_layers)?;
    lef(file, settings.exposure)?;
    lef(file, settings.bottom_exposure)?;
    for _ in 0..4 {
        le32(file, 0)?;
    }
    Ok(())
}

fn write_v4_slicer(file: &mut std::fs::File, settings: &PrintSettings) -> std::io::Result<()> {
    for v in [8.0, 300.0, 5.0, 300.0, 0.0, 300.0, 0.0] {
        lef(file, v)?;
    }
    le32(file, 1)?; // per-layer records are present
    let minutes = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0);
    le32(file, minutes as u32)?;
    le32(file, 1)?;
    le32(file, 0x0106_0300)?;
    for v in [
        settings.layer_move_s,
        0.0,
        settings.exposure,
        settings.bottom_exposure,
        0.0,
    ] {
        lef(file, v)?;
    }
    le32(file, 0)?; // transition layers
    le32(file, 0)?;
    le32(file, 0)
}

pub fn write_v4(
    path: &Path,
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    res: (usize, usize),
    progress: Progress<'_>,
) -> Result<Option<usize>, String> {
    let total = slice.layers.len();
    let header_size = 4 + MAGIC.len() + 2 + 4 + printer.machine_name.len() + 1 + 80;
    let params_offset = header_size as u32;
    let slicer_offset = params_offset + PRINT_PARAMETERS_SIZE as u32;
    let layers_offset = slicer_offset + SLICER_INFO_SIZE as u32;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| formats::io(path, e))?;
    file.write_all(&vec![0; header_size])
        .map_err(|e| formats::io(path, e))?;
    write_v4_params(&mut file, settings).map_err(|e| formats::io(path, e))?;
    write_v4_slicer(&mut file, settings).map_err(|e| formats::io(path, e))?;
    file.write_all(&vec![0; total * LAYER_DEF_SIZE])
        .map_err(|e| formats::io(path, e))?;

    let x1 = printer.machine_name.eq_ignore_ascii_case("HALOT-X1")
        || printer.machine_name.eq_ignore_ascii_case("R009")
        || res == (15120, 6230);
    let mut definitions = Vec::with_capacity(total);
    let mut buf = Vec::new();
    for (i, layer) in slice.layers.iter().enumerate() {
        formats::mask(layer, printer, res, &mut buf);
        let encoded = if x1 {
            pixel_pair_rle(&buf, res.0)?
        } else {
            rle7(&buf)
        };
        let data_address = file.stream_position().map_err(|e| formats::io(path, e))? as u32;
        let bottom = (i as u32) < settings.bottom_layers;
        for v in [
            8.0,
            300.0,
            5.0,
            300.0,
            300.0,
            0.0,
            0.0,
            0.0,
            0.0,
            settings.layer_move_s,
            255.0,
        ] {
            lef(&mut file, v).map_err(|e| formats::io(path, e))?;
        }
        file.write_all(&encoded).map_err(|e| formats::io(path, e))?;
        definitions.push((
            (i + 1) as f32 * slice.layer_height,
            if bottom {
                settings.bottom_exposure
            } else {
                settings.exposure
            },
            settings.layer_move_s,
            data_address,
            (LAYER_DEF_EX_SIZE + encoded.len()) as u32,
        ));
        if !progress(i + 1, total) {
            return Ok(None);
        }
    }

    file.seek(SeekFrom::Start(0))
        .map_err(|e| formats::io(path, e))?;
    write_v4_header(
        &mut file,
        printer,
        settings,
        slice,
        res,
        layers_offset,
        params_offset,
        slicer_offset,
    )
    .map_err(|e| formats::io(path, e))?;
    file.seek(SeekFrom::Start(layers_offset as u64))
        .map_err(|e| formats::io(path, e))?;
    for (z, exposure, off, address, size) in definitions {
        lef(&mut file, z).map_err(|e| formats::io(path, e))?;
        lef(&mut file, exposure).map_err(|e| formats::io(path, e))?;
        lef(&mut file, off).map_err(|e| formats::io(path, e))?;
        le32(&mut file, address).map_err(|e| formats::io(path, e))?;
        le32(&mut file, size).map_err(|e| formats::io(path, e))?;
        le32(&mut file, 0).map_err(|e| formats::io(path, e))?; // pixel data
        le32(&mut file, 0).map_err(|e| formats::io(path, e))?;
        le32(&mut file, 0).map_err(|e| formats::io(path, e))?;
        le32(&mut file, 0).map_err(|e| formats::io(path, e))?;
        le32(&mut file, 0).map_err(|e| formats::io(path, e))?;
    }
    file.seek(SeekFrom::End(0))
        .map_err(|e| formats::io(path, e))?;
    append_checksum(&mut file)?;
    Ok(Some(total))
}

fn utf16be_string(w: &mut impl Write, value: &str) -> std::io::Result<()> {
    be32(w, value.encode_utf16().count() as u32 * 2)?;
    for unit in value.encode_utf16() {
        be16(w, unit)?;
    }
    Ok(())
}

/// Legacy CXDLP v3 stores vertical non-black runs, not the v4 bitmap RLE.
pub fn write_v3(
    path: &Path,
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    res: (usize, usize),
    progress: Progress<'_>,
) -> Result<Option<usize>, String> {
    let total = slice.layers.len();
    if total > u16::MAX as usize || res.0 >= 16384 || res.1 >= 8192 {
        return Err("legacy CXDLP exceeds its coordinate or layer-count limits".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| formats::io(path, e))?;
    be32(&mut file, 9).map_err(|e| formats::io(path, e))?;
    file.write_all(MAGIC).map_err(|e| formats::io(path, e))?;
    be16(&mut file, 3).map_err(|e| formats::io(path, e))?;
    cstring_be(&mut file, &printer.machine_name).map_err(|e| formats::io(path, e))?;
    be16(&mut file, total as u16).map_err(|e| formats::io(path, e))?;
    be16(&mut file, res.0 as u16).map_err(|e| formats::io(path, e))?;
    be16(&mut file, res.1 as u16).map_err(|e| formats::io(path, e))?;
    file.write_all(&[0; 64]).map_err(|e| formats::io(path, e))?;
    // Three fixed-size RGB565 preview slots. Black previews are valid images;
    // omitting the slots would shift every subsequent record.
    for size in [116 * 116 * 2, 290 * 290 * 2, 290 * 290 * 2] {
        file.write_all(&vec![0; size])
            .map_err(|e| formats::io(path, e))?;
        file.write_all(b"\r\n").map_err(|e| formats::io(path, e))?;
    }
    utf16be_string(&mut file, &format!("{}", printer.build[0]))
        .map_err(|e| formats::io(path, e))?;
    utf16be_string(&mut file, &format!("{}", printer.build[1]))
        .map_err(|e| formats::io(path, e))?;
    utf16be_string(&mut file, &format!("{}", slice.layer_height))
        .map_err(|e| formats::io(path, e))?;
    for value in [
        // Exposure is in deciseconds. CL-60 speeds are integer mm/s;
        // UVtools rounds the reference 80/80/150 mm/min to 1/1/3 here.
        (settings.exposure.max(0.0) * 10.0).round() as u16,
        settings.layer_move_s.ceil().max(1.0) as u16,
        settings.bottom_exposure.round().max(0.0) as u16,
        settings.bottom_layers.min(u16::MAX as u32) as u16,
        7,
        1,
        6,
        1,
        3,
        255,
        255,
    ] {
        be16(&mut file, value).map_err(|e| formats::io(path, e))?;
    }
    let area_table = file.stream_position().map_err(|e| formats::io(path, e))?;
    file.write_all(&vec![0; total * 4])
        .map_err(|e| formats::io(path, e))?;
    file.write_all(b"\r\n").map_err(|e| formats::io(path, e))?;
    cstring_be(&mut file, "Pinto Slicer 0.1").map_err(|e| formats::io(path, e))?;
    cstring_be(&mut file, "").map_err(|e| formats::io(path, e))?;
    file.write_all(&[0]).map_err(|e| formats::io(path, e))?;
    be32(&mut file, 600).map_err(|e| formats::io(path, e))?;
    be32(&mut file, 300000).map_err(|e| formats::io(path, e))?;
    file.write_all(&[1]).map_err(|e| formats::io(path, e))?;
    be16(&mut file, 0).map_err(|e| formats::io(path, e))?;
    file.write_all(&[0]).map_err(|e| formats::io(path, e))?;
    be16(&mut file, 1000).map_err(|e| formats::io(path, e))?;
    file.write_all(&[1, 1, 255, 0, 2])
        .map_err(|e| formats::io(path, e))?;
    file.write_all(b"\r\n").map_err(|e| formats::io(path, e))?;

    let mut areas = Vec::with_capacity(total);
    let mut buf = Vec::new();
    let pixel_area = printer.build[0] / res.0 as f32 * printer.build[1] / res.1 as f32;
    for (i, layer) in slice.layers.iter().enumerate() {
        formats::mask(layer, printer, res, &mut buf);
        let lit = buf.iter().filter(|v| **v != 0).count();
        areas.push((lit as f32 * pixel_area * 1000.0).round() as u32);
        let mut lines = Vec::new();
        for x in 0..res.0 {
            let mut y = 0;
            while y < res.1 {
                let gray = buf[y * res.0 + x];
                let start = y;
                y += 1;
                while y < res.1 && buf[y * res.0 + x] == gray {
                    y += 1;
                }
                if gray == 0 {
                    continue;
                }
                let start = start as u16;
                let end = (y - 1) as u16;
                let x = x as u16;
                lines.extend_from_slice(&[
                    ((start >> 5) & 0xff) as u8,
                    (((start << 3) + (end >> 10)) & 0xff) as u8,
                    ((end >> 2) & 0xff) as u8,
                    (((end << 6) + (x >> 8)) & 0xff) as u8,
                    x as u8,
                    gray,
                ]);
            }
        }
        be32(&mut file, areas[i]).map_err(|e| formats::io(path, e))?;
        be32(&mut file, (lines.len() / 6) as u32).map_err(|e| formats::io(path, e))?;
        file.write_all(&lines).map_err(|e| formats::io(path, e))?;
        file.write_all(b"\r\n").map_err(|e| formats::io(path, e))?;
        if !progress(i + 1, total) {
            return Ok(None);
        }
    }
    file.seek(SeekFrom::Start(area_table))
        .map_err(|e| formats::io(path, e))?;
    for area in areas {
        be32(&mut file, area).map_err(|e| formats::io(path, e))?;
    }
    file.seek(SeekFrom::End(0))
        .map_err(|e| formats::io(path, e))?;
    be32(&mut file, 9).map_err(|e| formats::io(path, e))?;
    file.write_all(MAGIC).map_err(|e| formats::io(path, e))?;
    append_checksum(&mut file)?;
    Ok(Some(total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_printers;
    use crate::mesh;
    use crate::slicer;

    #[test]
    fn pixel_pairs_round_trip_and_never_cross_rows() {
        let pixels = [0, 0, 0, 0, 255, 255, 0, 0, 224, 224, 224, 224];
        let rle = pixel_pair_rle(&pixels, 4).unwrap();
        let expected = [0, 0, 0, 0, 255, 255, 0, 0, 255, 255, 255, 255];
        assert_eq!(decode_pixel_pair(&rle, 4, 3).unwrap(), expected);
        assert!(pixel_pair_rle(&pixels, 3).unwrap_err().contains("even"));
    }

    #[test]
    fn seven_bit_rle_handles_long_runs() {
        let input = vec![255u8; 400];
        let rle = rle7(&input);
        assert_eq!(rle[0], 0xff);
        assert!(rle.len() < 10);
    }

    #[test]
    fn v4_has_the_real_header_layer_records_and_crc() {
        let sl = slicer::slice(&[mesh::sample_block()], 0.5);
        let path = std::env::temp_dir().join("resin_slicer_real.cxdlpv4");
        let n = write_v4(
            &path,
            &default_printers()[0],
            &PrintSettings::default(),
            &sl,
            (96, 64),
            &mut |_, _| true,
        )
        .unwrap()
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(u32::from_be_bytes(bytes[0..4].try_into().unwrap()), 9);
        assert_eq!(&bytes[4..13], MAGIC);
        assert_eq!(u16::from_be_bytes(bytes[13..15].try_into().unwrap()), 4);
        assert_eq!(u32::from_be_bytes(bytes[15..19].try_into().unwrap()), 9);
        assert_eq!(&bytes[19..28], b"HALOT-X1\0");
        assert_eq!(u16::from_le_bytes(bytes[28..30].try_into().unwrap()), 96);
        let layer_table = u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
        let data = u32::from_le_bytes(
            bytes[layer_table + 12..layer_table + 16]
                .try_into()
                .unwrap(),
        ) as usize;
        let size = u32::from_le_bytes(
            bytes[layer_table + 16..layer_table + 20]
                .try_into()
                .unwrap(),
        ) as usize;
        let decoded =
            decode_pixel_pair(&bytes[data + LAYER_DEF_EX_SIZE..data + size], 96, 64).unwrap();
        assert_eq!(decoded.len(), 96 * 64);
        let stored_crc = u32::from_be_bytes(bytes[bytes.len() - 4..].try_into().unwrap());
        assert_eq!(crc32(&bytes[..bytes.len() - 4], 0), stored_crc);
        assert_eq!(n, sl.layers.len());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn v3_has_big_endian_magic_and_a_valid_crc() {
        let sl = slicer::slice(&[mesh::sample_block()], 0.5);
        let path = std::env::temp_dir().join("resin_slicer_real.cxdlp");
        write_v3(
            &path,
            &default_printers()[1],
            &PrintSettings::default(),
            &sl,
            (96, 64),
            &mut |_, _| true,
        )
        .unwrap()
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[4..13], MAGIC);
        assert_eq!(u16::from_be_bytes(bytes[13..15].try_into().unwrap()), 3);
        // Legacy motion uses integer mm/s and exposure uses deciseconds.
        let motion: Vec<u8> = [25_u16, 4, 35, 5, 7, 1, 6, 1, 3, 255, 255]
            .into_iter()
            .flat_map(u16::to_be_bytes)
            .collect();
        assert!(bytes.windows(motion.len()).any(|w| w == motion));
        let stored_crc = u32::from_be_bytes(bytes[bytes.len() - 4..].try_into().unwrap());
        assert_eq!(crc32(&bytes[..bytes.len() - 4], 0), stored_crc);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    #[ignore = "allocates and encodes the 94-megapixel HALOT-X1 panel"]
    fn halot_x1_writes_its_exact_native_panel() {
        let source = slicer::slice(&[mesh::sample_block()], 0.5);
        let sl = crate::slicer::SliceResult {
            layers: vec![source.layers[0].clone()],
            layer_height: 0.5,
            height: 0.5,
        };
        let printer = &default_printers()[0];
        let path = std::env::temp_dir().join("resin_slicer_native_x1.cxdlpv4");
        write_v4(
            &path,
            printer,
            &PrintSettings::default(),
            &sl,
            printer.native_res(),
            &mut |_, _| true,
        )
        .unwrap()
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(u16::from_le_bytes(bytes[28..30].try_into().unwrap()), 15120);
        assert_eq!(u16::from_le_bytes(bytes[30..32].try_into().unwrap()), 6230);
        if std::env::var_os("KEEP_FORMAT_SAMPLES").is_none() {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn cancellation_stops_before_a_complete_job() {
        let sl = slicer::slice(&[mesh::sample_block()], 0.5);
        let path = std::env::temp_dir().join("resin_slicer_stop.cxdlpv4");
        let result = write_v4(
            &path,
            &default_printers()[0],
            &PrintSettings::default(),
            &sl,
            (96, 64),
            &mut |i, _| i < 2,
        )
        .unwrap();
        assert_eq!(result, None);
        let _ = std::fs::remove_file(path);
    }
}
