//! Anycubic Photon-family and legacy ChiTu Photon writers.
//!
//! `.pws` uses Anycubic v1 bit planes. `.pwmo`, `.pwmx`, `.pm3` and `.pwma`
//! use the v516 layout and PW0 high-nibble RLE. `.photon` is CBDDLP v2, a
//! different header and bit-plane format (not a renamed CTB file).

use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use crate::config::{JobFormat, PrintSettings, PrinterProfile};
use crate::slicer::SliceResult;

use super::{mask, Progress};

fn u16le(w: &mut impl Write, v: u16) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn u32le(w: &mut impl Write, v: u32) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn f32le(w: &mut impl Write, v: f32) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn fixed(w: &mut impl Write, text: &str, size: usize) -> std::io::Result<()> {
    let mut bytes = vec![0u8; size];
    let n = text.len().min(size);
    bytes[..n].copy_from_slice(&text.as_bytes()[..n]);
    w.write_all(&bytes)
}

fn bit_plane_rle(mask: &[u8], threshold: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(mask.len() / 32 + 16);
    let mut at = 0;
    while at < mask.len() {
        let on = mask[at] >= threshold;
        let mut end = at + 1;
        while end < mask.len() && (mask[end] >= threshold) == on {
            end += 1;
        }
        let mut left = end - at;
        while left > 0 {
            let run = left.min(125);
            out.push(run as u8 | if on { 0x80 } else { 0 });
            left -= run;
        }
        at = end;
    }
    out
}

fn pw0_rle(mask: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(mask.len() / 16 + 16);
    let mut at = 0;
    while at < mask.len() {
        let colour = mask[at] >> 4;
        let mut end = at + 1;
        while end < mask.len() && mask[end] >> 4 == colour {
            end += 1;
        }
        let mut left = end - at;
        while left > 0 {
            let limit = if colour == 0 || colour == 15 {
                4095
            } else {
                15
            };
            let run = left.min(limit);
            if colour == 0 || colour == 15 {
                let word = (run as u16) | ((colour as u16) << 12);
                out.extend_from_slice(&word.to_be_bytes());
            } else {
                out.push((colour << 4) | run as u8);
            }
            left -= run;
        }
        at = end;
    }
    out
}

fn thumbnail(
    slice: &SliceResult,
    printer: &PrinterProfile,
    res: (usize, usize),
    size: (u32, u32),
) -> image::GrayImage {
    let Some(layer) = slice.layers.iter().max_by_key(|layer| layer.segs.len()) else {
        return image::GrayImage::new(size.0, size.1);
    };
    let mut buf = Vec::new();
    mask(layer, printer, res, &mut buf);
    let image = image::GrayImage::from_raw(res.0 as u32, res.1 as u32, buf)
        .unwrap_or_else(|| image::GrayImage::new(res.0 as u32, res.1 as u32));
    image::imageops::resize(
        &image,
        size.0,
        size.1,
        image::imageops::FilterType::Triangle,
    )
}

fn write_anycubic_preview(
    file: &mut std::fs::File,
    image: &image::GrayImage,
) -> Result<(), String> {
    fixed(file, "PREVIEW", 12).map_err(|e| e.to_string())?;
    u32le(file, 28 + image.width() * image.height() * 2).map_err(|e| e.to_string())?;
    u32le(file, image.width()).map_err(|e| e.to_string())?;
    file.write_all(b"x\0\0\0").map_err(|e| e.to_string())?;
    u32le(file, image.height()).map_err(|e| e.to_string())?;
    for p in image.pixels() {
        let v = p[0] as u16;
        let rgb565 = ((v & 0xf8) << 8) | ((v & 0xfc) << 3) | (v >> 3);
        u16le(file, rgb565).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn write_anycubic_header(
    file: &mut std::fs::File,
    v516: bool,
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    res: (usize, usize),
) -> Result<(), String> {
    fixed(file, "HEADER", 12).map_err(|e| e.to_string())?;
    u32le(file, if v516 { 84 } else { 80 }).map_err(|e| e.to_string())?;
    let px = (printer.build[0] * 1_000_000.0 / res.0 as f32).round() / 1000.0;
    let py = (printer.build[1] * 1_000_000.0 / res.1 as f32).round() / 1000.0;
    for value in [
        px.max(py),
        slice.layer_height,
        settings.exposure,
        settings.layer_move_s,
        settings.bottom_exposure,
        settings.bottom_layers as f32,
        5.0,
        5.0,
        5.0,
        0.0,
    ] {
        f32le(file, value).map_err(|e| e.to_string())?;
    }
    u32le(file, 1).map_err(|e| e.to_string())?;
    u32le(file, res.0 as u32).map_err(|e| e.to_string())?;
    u32le(file, res.1 as u32).map_err(|e| e.to_string())?;
    f32le(file, 0.0).map_err(|e| e.to_string())?;
    f32le(file, 0.0).map_err(|e| e.to_string())?;
    file.write_all(b"$\0\0\0").map_err(|e| e.to_string())?;
    u32le(file, 1).map_err(|e| e.to_string())?;
    u32le(
        file,
        (settings.estimated_minutes(slice.layers.len()) * 60.0) as u32,
    )
    .map_err(|e| e.to_string())?;
    u32le(file, 0).map_err(|e| e.to_string())?;
    u32le(file, 0).map_err(|e| e.to_string())?;
    if v516 {
        u32le(file, 0).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_anycubic_mark(
    file: &mut std::fs::File,
    v516: bool,
    preview: u32,
    colour: u32,
    layerdef: u32,
    extra: u32,
    machine: u32,
    images: u32,
) -> Result<(), String> {
    fixed(file, "ANYCUBIC", 12).map_err(|e| e.to_string())?;
    u32le(file, if v516 { 516 } else { 1 }).map_err(|e| e.to_string())?;
    u32le(file, if v516 { 8 } else { 4 }).map_err(|e| e.to_string())?;
    u32le(file, if v516 { 52 } else { 48 }).map_err(|e| e.to_string())?;
    u32le(file, 0).map_err(|e| e.to_string())?;
    u32le(file, preview).map_err(|e| e.to_string())?;
    u32le(file, colour).map_err(|e| e.to_string())?;
    u32le(file, layerdef).map_err(|e| e.to_string())?;
    u32le(file, extra).map_err(|e| e.to_string())?;
    if v516 {
        u32le(file, machine).map_err(|e| e.to_string())?;
    }
    u32le(file, images).map_err(|e| e.to_string())
}

fn write_extra(file: &mut std::fs::File) -> Result<(), String> {
    fixed(file, "EXTRA", 12).map_err(|e| e.to_string())?;
    u32le(file, 24).map_err(|e| e.to_string())?; // deliberate format quirk
    u32le(file, 2).map_err(|e| e.to_string())?;
    for value in [8.0, 5.0, 5.0, 0.0, 5.0, 5.0] {
        f32le(file, value).map_err(|e| e.to_string())?;
    }
    u32le(file, 2).map_err(|e| e.to_string())?;
    for value in [5.0, 5.0, 5.0, 0.0, 5.0, 5.0] {
        f32le(file, value).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn write_machine(
    file: &mut std::fs::File,
    printer: &PrinterProfile,
    format: JobFormat,
) -> Result<(), String> {
    fixed(file, "MACHINE", 12).map_err(|e| e.to_string())?;
    u32le(file, 156).map_err(|e| e.to_string())?;
    fixed(file, &printer.machine_name, 96).map_err(|e| e.to_string())?;
    fixed(
        file,
        if format == JobFormat::Pws {
            "pwsImg"
        } else {
            "pw0Img"
        },
        16,
    )
    .map_err(|e| e.to_string())?;
    u32le(file, 16).map_err(|e| e.to_string())?;
    u32le(file, 1).map_err(|e| e.to_string())?;
    f32le(file, printer.build[0]).map_err(|e| e.to_string())?;
    f32le(file, printer.build[1]).map_err(|e| e.to_string())?;
    f32le(file, printer.build[2]).map_err(|e| e.to_string())?;
    u32le(file, 516).map_err(|e| e.to_string())?;
    u32le(file, 6_506_241).map_err(|e| e.to_string())
}

#[derive(Clone, Copy)]
struct LayerEntry {
    address: u32,
    length: u32,
    exposure: f32,
    height: f32,
    nonzero: u32,
}

fn write_anycubic(
    format: JobFormat,
    path: &Path,
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    res: (usize, usize),
    progress: Progress<'_>,
) -> Result<Option<usize>, String> {
    let total = slice.layers.len();
    let v516 = format != JobFormat::Pws;
    let mark_size = if v516 { 52 } else { 48 };
    let header_size = if v516 { 100 } else { 96 };
    let preview_address = mark_size + header_size;
    let colour_address = preview_address + 75_292;
    let layerdef_address = colour_address + if v516 { 28 } else { 0 };
    let layerdef_size = 20 + 32 * total;
    let extra_address = layerdef_address + layerdef_size;
    let machine_address = extra_address + if v516 { 72 } else { 0 };
    let images_address = machine_address + if v516 { 156 } else { 0 };

    let mut file = std::fs::File::create(path).map_err(|e| super::io(path, e))?;
    file.write_all(&vec![0; mark_size])
        .map_err(|e| super::io(path, e))?;
    write_anycubic_header(&mut file, v516, printer, settings, slice, res)
        .map_err(|e| super::io(path, e))?;
    let preview = thumbnail(slice, printer, res, (224, 168));
    write_anycubic_preview(&mut file, &preview).map_err(|e| super::io(path, e))?;
    if v516 {
        u32le(&mut file, 0).map_err(|e| super::io(path, e))?;
        u32le(&mut file, 16).map_err(|e| super::io(path, e))?;
        file.write_all(&[255; 16]).map_err(|e| super::io(path, e))?;
        u32le(&mut file, 0).map_err(|e| super::io(path, e))?;
    }
    debug_assert_eq!(file.stream_position().unwrap() as usize, layerdef_address);
    file.write_all(&vec![0; layerdef_size])
        .map_err(|e| super::io(path, e))?;
    if v516 {
        write_extra(&mut file).map_err(|e| super::io(path, e))?;
        write_machine(&mut file, printer, format).map_err(|e| super::io(path, e))?;
    }
    debug_assert_eq!(file.stream_position().unwrap() as usize, images_address);

    let mut entries = Vec::with_capacity(total);
    let mut buf = Vec::new();
    for (i, layer) in slice.layers.iter().enumerate() {
        mask(layer, printer, res, &mut buf);
        let encoded = if format == JobFormat::Pws {
            bit_plane_rle(&buf, 128)
        } else {
            pw0_rle(&buf)
        };
        let address = file.stream_position().map_err(|e| super::io(path, e))? as u32;
        file.write_all(&encoded).map_err(|e| super::io(path, e))?;
        entries.push(LayerEntry {
            address,
            length: encoded.len() as u32,
            exposure: if (i as u32) < settings.bottom_layers {
                settings.bottom_exposure
            } else {
                settings.exposure
            },
            height: slice.layer_height,
            nonzero: buf.iter().filter(|v| **v != 0).count() as u32,
        });
        if !progress(i + 1, total) {
            return Ok(None);
        }
    }

    file.seek(SeekFrom::Start(layerdef_address as u64))
        .map_err(|e| super::io(path, e))?;
    fixed(&mut file, "LAYERDEF", 12).map_err(|e| super::io(path, e))?;
    u32le(&mut file, (4 + 32 * total) as u32).map_err(|e| super::io(path, e))?;
    u32le(&mut file, total as u32).map_err(|e| super::io(path, e))?;
    for entry in entries {
        u32le(&mut file, entry.address).map_err(|e| super::io(path, e))?;
        u32le(&mut file, entry.length).map_err(|e| super::io(path, e))?;
        f32le(&mut file, 5.0).map_err(|e| super::io(path, e))?;
        f32le(&mut file, 5.0).map_err(|e| super::io(path, e))?;
        f32le(&mut file, entry.exposure).map_err(|e| super::io(path, e))?;
        f32le(&mut file, entry.height).map_err(|e| super::io(path, e))?;
        u32le(&mut file, entry.nonzero).map_err(|e| super::io(path, e))?;
        u32le(&mut file, 0).map_err(|e| super::io(path, e))?;
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|e| super::io(path, e))?;
    write_anycubic_mark(
        &mut file,
        v516,
        preview_address as u32,
        if v516 { colour_address as u32 } else { 0 },
        layerdef_address as u32,
        if v516 { extra_address as u32 } else { 0 },
        if v516 { machine_address as u32 } else { 0 },
        images_address as u32,
    )
    .map_err(|e| super::io(path, e))?;
    file.flush().map_err(|e| super::io(path, e))?;
    Ok(Some(total))
}

fn photon_preview_rle(image: &image::GrayImage) -> Vec<u8> {
    let keys: Vec<u16> = image
        .pixels()
        .map(|p| {
            let v = p[0] as u16;
            (v >> 3) | ((v >> 2) << 5) | ((v >> 3) << 11)
        })
        .collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < keys.len() {
        let key = keys[at];
        let mut end = at + 1;
        while end < keys.len() && keys[end] == key && end - at < 4095 {
            end += 1;
        }
        let run = end - at;
        match run {
            1 => out.extend_from_slice(&(key & !0x20).to_le_bytes()),
            2 => {
                out.extend_from_slice(&(key & !0x20).to_le_bytes());
                out.extend_from_slice(&(key & !0x20).to_le_bytes());
            }
            _ => {
                out.extend_from_slice(&(key | 0x20).to_le_bytes());
                out.extend_from_slice(&(((run - 1) as u16) | 0x3000).to_le_bytes());
            }
        }
        at = end;
    }
    out
}

fn write_photon_preview(
    file: &mut std::fs::File,
    image: &image::GrayImage,
) -> Result<usize, String> {
    let data = photon_preview_rle(image);
    let image_offset = file.stream_position().map_err(|e| e.to_string())? as u32 + 32;
    for value in [
        image.width(),
        image.height(),
        image_offset,
        data.len() as u32,
        0,
        0,
        0,
        0,
    ] {
        u32le(file, value).map_err(|e| e.to_string())?;
    }
    file.write_all(&data).map_err(|e| e.to_string())?;
    Ok(data.len())
}

#[allow(clippy::too_many_arguments)]
fn write_photon_header(
    file: &mut std::fs::File,
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    res: (usize, usize),
    large: u32,
    small: u32,
    params: u32,
    layerdef: u32,
) -> Result<(), String> {
    u32le(file, 0x12FD0019).map_err(|e| e.to_string())?;
    u32le(file, 2).map_err(|e| e.to_string())?;
    for v in [printer.build[0], printer.build[1], printer.build[2]] {
        f32le(file, v).map_err(|e| e.to_string())?;
    }
    u32le(file, 0).map_err(|e| e.to_string())?;
    u32le(file, 0).map_err(|e| e.to_string())?;
    for v in [
        slice.height,
        slice.layer_height,
        settings.exposure,
        settings.bottom_exposure,
        settings.layer_move_s,
    ] {
        f32le(file, v).map_err(|e| e.to_string())?;
    }
    u32le(file, settings.bottom_layers).map_err(|e| e.to_string())?;
    u32le(file, res.0 as u32).map_err(|e| e.to_string())?;
    u32le(file, res.1 as u32).map_err(|e| e.to_string())?;
    u32le(file, large).map_err(|e| e.to_string())?;
    u32le(file, layerdef).map_err(|e| e.to_string())?;
    u32le(file, slice.layers.len() as u32).map_err(|e| e.to_string())?;
    u32le(file, small).map_err(|e| e.to_string())?;
    u32le(
        file,
        (settings.estimated_minutes(slice.layers.len()) * 60.0) as u32,
    )
    .map_err(|e| e.to_string())?;
    u32le(file, printer.mirror[0] as u32).map_err(|e| e.to_string())?;
    u32le(file, params).map_err(|e| e.to_string())?;
    u32le(file, 60).map_err(|e| e.to_string())?;
    u32le(file, 1).map_err(|e| e.to_string())?;
    u16le(file, 255).map_err(|e| e.to_string())?;
    u16le(file, 255).map_err(|e| e.to_string())?;
    u32le(file, 0).map_err(|e| e.to_string())?;
    u32le(file, 0).map_err(|e| e.to_string())?;
    u32le(file, 0).map_err(|e| e.to_string())
}

fn write_photon(
    path: &Path,
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    res: (usize, usize),
    progress: Progress<'_>,
) -> Result<Option<usize>, String> {
    let total = slice.layers.len();
    let mut file = std::fs::File::create(path).map_err(|e| super::io(path, e))?;
    file.write_all(&[0; 112]).map_err(|e| super::io(path, e))?;
    let large_offset = 112u32;
    let large = thumbnail(slice, printer, res, (400, 300));
    let large_len = write_photon_preview(&mut file, &large).map_err(|e| super::io(path, e))?;
    let small_offset = 112 + 32 + large_len as u32;
    let small = thumbnail(slice, printer, res, (200, 125));
    let small_len = write_photon_preview(&mut file, &small).map_err(|e| super::io(path, e))?;
    let params_offset = small_offset + 32 + small_len as u32;
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
        f32le(&mut file, v).map_err(|e| super::io(path, e))?;
    }
    u32le(&mut file, settings.bottom_layers).map_err(|e| super::io(path, e))?;
    for _ in 0..4 {
        u32le(&mut file, 0).map_err(|e| super::io(path, e))?;
    }
    let layerdef_offset = params_offset + 60;
    file.write_all(&vec![0; total * 36])
        .map_err(|e| super::io(path, e))?;
    let mut entries = Vec::with_capacity(total);
    let mut buf = Vec::new();
    for (i, layer) in slice.layers.iter().enumerate() {
        mask(layer, printer, res, &mut buf);
        let encoded = bit_plane_rle(&buf, 127);
        let address = file.stream_position().map_err(|e| super::io(path, e))? as u32;
        file.write_all(&encoded).map_err(|e| super::io(path, e))?;
        entries.push(LayerEntry {
            address,
            length: encoded.len() as u32,
            exposure: if (i as u32) < settings.bottom_layers {
                settings.bottom_exposure
            } else {
                settings.exposure
            },
            height: (i + 1) as f32 * slice.layer_height,
            nonzero: 0,
        });
        if !progress(i + 1, total) {
            return Ok(None);
        }
    }
    file.seek(SeekFrom::Start(layerdef_offset as u64))
        .map_err(|e| super::io(path, e))?;
    for entry in entries {
        f32le(&mut file, entry.height).map_err(|e| super::io(path, e))?;
        f32le(&mut file, entry.exposure).map_err(|e| super::io(path, e))?;
        f32le(&mut file, settings.layer_move_s).map_err(|e| super::io(path, e))?;
        u32le(&mut file, entry.address).map_err(|e| super::io(path, e))?;
        u32le(&mut file, entry.length).map_err(|e| super::io(path, e))?;
        u32le(&mut file, 0).map_err(|e| super::io(path, e))?;
        u32le(&mut file, 36).map_err(|e| super::io(path, e))?;
        u32le(&mut file, 0).map_err(|e| super::io(path, e))?;
        u32le(&mut file, 0).map_err(|e| super::io(path, e))?;
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|e| super::io(path, e))?;
    write_photon_header(
        &mut file,
        printer,
        settings,
        slice,
        res,
        large_offset,
        small_offset,
        params_offset,
        layerdef_offset,
    )
    .map_err(|e| super::io(path, e))?;
    file.flush().map_err(|e| super::io(path, e))?;
    Ok(Some(total))
}

pub(super) fn write_job(
    format: JobFormat,
    path: &Path,
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    res: (usize, usize),
    progress: Progress<'_>,
) -> Result<Option<usize>, String> {
    if format == JobFormat::Photon {
        write_photon(path, printer, settings, slice, res, progress)
    } else {
        write_anycubic(format, path, printer, settings, slice, res, progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_planes_cover_every_pixel_and_split_at_125() {
        let rle = bit_plane_rle(&vec![255; 300], 128);
        assert_eq!(rle, vec![0xfd, 0xfd, 0xb2]);
        let black = bit_plane_rle(&[0; 126], 128);
        assert_eq!(black, vec![125, 1]);
    }

    #[test]
    fn pw0_uses_big_endian_words_for_extreme_colours() {
        assert_eq!(pw0_rle(&vec![0; 300]), vec![1, 44]);
        assert_eq!(pw0_rle(&[255; 17]), vec![0xf0, 0x11]);
        assert_eq!(pw0_rle(&[0x88; 16]), vec![0x8f, 0x81]);
    }

    #[test]
    fn black_photon_preview_uses_repeat_words() {
        let rle = photon_preview_rle(&image::GrayImage::new(4, 1));
        assert_eq!(rle, vec![0x20, 0, 0x03, 0x30]);
    }

    #[test]
    fn anycubic_v1_and_v516_layouts_have_the_right_tables() {
        let sl = crate::slicer::slice(&[crate::mesh::sample_block()], 0.5);
        for (format, index) in [(JobFormat::Pws, 4), (JobFormat::Pwmo, 5)] {
            let mut printer = crate::config::default_printers()[index].clone();
            printer.format = format;
            let path =
                std::env::temp_dir().join(format!("resin_slicer_test.{}", format.extension()));
            write_job(
                format,
                &path,
                &printer,
                &PrintSettings::default(),
                &sl,
                (96, 64),
                &mut |_, _| true,
            )
            .unwrap()
            .unwrap();
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(&bytes[..8], b"ANYCUBIC");
            let version = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
            assert_eq!(version, if format == JobFormat::Pws { 1 } else { 516 });
            let layerdef = u32::from_le_bytes(bytes[36..40].try_into().unwrap()) as usize;
            assert_eq!(&bytes[layerdef..layerdef + 8], b"LAYERDEF");
            let images = u32::from_le_bytes(
                bytes[if version == 1 { 44 } else { 48 }..][..4]
                    .try_into()
                    .unwrap(),
            ) as usize;
            let address =
                u32::from_le_bytes(bytes[layerdef + 20..layerdef + 24].try_into().unwrap())
                    as usize;
            assert_eq!(address, images);
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn photon_is_cbddlp_v2_not_ctb() {
        let sl = crate::slicer::slice(&[crate::mesh::sample_block()], 0.5);
        let path = std::env::temp_dir().join("resin_slicer_test.photon");
        write_job(
            JobFormat::Photon,
            &path,
            &crate::config::default_printers()[4],
            &PrintSettings::default(),
            &sl,
            (96, 64),
            &mut |_, _| true,
        )
        .unwrap()
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], &[0x19, 0, 0xfd, 0x12, 2, 0, 0, 0]);
        let layerdef = u32::from_le_bytes(bytes[64..68].try_into().unwrap()) as usize;
        assert_eq!(
            u32::from_le_bytes(bytes[layerdef + 24..layerdef + 28].try_into().unwrap()),
            36
        );
        let _ = std::fs::remove_file(path);
    }
}
