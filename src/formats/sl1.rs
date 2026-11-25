//! Prusa SL1/SL1S ZIP archives: INI metadata, PNG layers and thumbnails.

use std::io::Write;
use std::path::Path;

use zip::write::SimpleFileOptions;

use crate::config::{JobFormat, PrintSettings, PrinterProfile};
use crate::slicer::SliceResult;

use super::{mask, Progress};

fn png(image: &image::GrayImage) -> Result<Vec<u8>, String> {
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("could not encode a PNG layer: {e}"))?;
    Ok(out.into_inner())
}

fn entry(
    zip: &mut zip::ZipWriter<std::fs::File>,
    options: SimpleFileOptions,
    name: &str,
    bytes: &[u8],
    path: &Path,
) -> Result<(), String> {
    zip.start_file(name, options)
        .map_err(|e| super::io(path, e))?;
    zip.write_all(bytes).map_err(|e| super::io(path, e))
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
    let total = slice.layers.len();
    let bottom_layers = settings
        .bottom_layers
        .min(total as u32)
        .min(u16::MAX as u32);
    debug_assert!(matches!(format, JobFormat::Sl1 | JobFormat::Sl1s));
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("job");
    let model = printer.machine_name.as_str();
    let created = chrono::Utc::now().format("%Y-%m-%d at %H:%M:%S UTC");
    let config = format!(
        "action = print\njobDir = {stem}\nexpTime = {:.4}\nexpTimeFirst = {:.4}\nexpUserProfile = 0\nfileCreationTimestamp = {created}\nhollow = 0\nlayerHeight = {:.4}\nmaterialName = Standard resin\nnumFade = {bottom_layers}\nnumFast = {total}\nnumSlow = 0\nprintProfile = Pinto Slicer\nprintTime = {:.3}\nprinterModel = {model}\nprinterProfile = {}\nprinterVariant = default\nprusaSlicerVersion = Pinto-Slicer-0.1.0\nusedMaterial = 0\n",
        settings.exposure,
        settings.bottom_exposure,
        slice.layer_height,
        settings.estimated_minutes(total) * 60.0,
        printer.name,
    );
    let prusa = format!(
        "printer_model = {model}\nprinter_technology = SLA\nmax_print_height = {:.4}\ndisplay_width = {:.6}\ndisplay_height = {:.6}\ndisplay_pixels_x = {}\ndisplay_pixels_y = {}\ndisplay_mirror_x = {}\ndisplay_mirror_y = {}\ndisplay_orientation = landscape\nsla_archive_format = {}\nlayer_height = {:.4}\ninitial_layer_height = {:.4}\nexposure_time = {:.4}\ninitial_exposure_time = {:.4}\nfaded_layers = {bottom_layers}\nsupports_enable = {}\npad_enable = {}\nfast_tilt_time = 5\nslow_tilt_time = 8\nhigh_viscosity_tilt_time = 10\nrelative_correction = 1,1\ngamma_correction = 1\n",
        printer.build[2],
        printer.build[0],
        printer.build[1],
        res.0,
        res.1,
        printer.mirror[0] as u8,
        printer.mirror[1] as u8,
        "SL1",
        slice.layer_height,
        slice.layer_height,
        settings.exposure,
        settings.bottom_exposure,
        settings.supports.enabled as u8,
        settings.supports.base.enabled as u8,
    );

    let file = std::fs::File::create(path).map_err(|e| super::io(path, e))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    entry(&mut zip, options, "config.ini", config.as_bytes(), path)?;
    entry(&mut zip, options, "prusaslicer.ini", prusa.as_bytes(), path)?;

    let mut buf = Vec::new();
    let mut thumbnail: Option<image::GrayImage> = None;
    for (i, layer) in slice.layers.iter().enumerate() {
        mask(layer, printer, res, &mut buf);
        let image = image::GrayImage::from_raw(res.0 as u32, res.1 as u32, buf.clone())
            .ok_or_else(|| "could not build a layer picture".to_string())?;
        if thumbnail.is_none() && buf.iter().any(|p| *p != 0) {
            thumbnail = Some(image.clone());
        }
        let bytes = png(&image)?;
        entry(
            &mut zip,
            options,
            &format!("{stem}{i:05}.png"),
            &bytes,
            path,
        )?;
        if !progress(i + 1, total) {
            return Ok(None);
        }
    }

    let thumb = thumbnail.unwrap_or_else(|| image::GrayImage::new(res.0 as u32, res.1 as u32));
    for (w, h) in [(400, 400), (800, 480)] {
        let image = image::imageops::resize(&thumb, w, h, image::imageops::FilterType::Triangle);
        let bytes = png(&image)?;
        entry(
            &mut zip,
            options,
            &format!("thumbnail/thumbnail{w}x{h}.png"),
            &bytes,
            path,
        )?;
    }
    zip.finish().map_err(|e| super::io(path, e))?;
    Ok(Some(total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_printers;
    use crate::mesh;
    use std::io::Read;

    #[test]
    fn sl1s_uses_the_firmware_model_identifier() {
        let sl = crate::slicer::slice(&[mesh::sample_block()], 0.5);
        let path = std::env::temp_dir().join("resin_slicer_test.sl1s");
        write_job(
            JobFormat::Sl1s,
            &path,
            &default_printers()[9],
            &PrintSettings::default(),
            &sl,
            (96, 64),
            &mut |_, _| true,
        )
        .unwrap()
        .unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        for (name, model_line) in [
            ("config.ini", "printerModel = SL1S\n"),
            ("prusaslicer.ini", "printer_model = SL1S\n"),
        ] {
            let mut text = String::new();
            archive
                .by_name(name)
                .unwrap()
                .read_to_string(&mut text)
                .unwrap();
            assert!(text.contains(model_line), "{name}: {text}");
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn sl1_is_a_complete_indexed_png_archive() {
        let sl = crate::slicer::slice(&[mesh::sample_block()], 0.5);
        let path = std::env::temp_dir().join("resin_slicer_test.sl1");
        write_job(
            JobFormat::Sl1,
            &path,
            &default_printers()[8],
            &PrintSettings::default(),
            &sl,
            (96, 64),
            &mut |_, _| true,
        )
        .unwrap()
        .unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(archive.len(), sl.layers.len() + 4);
        let mut config = String::new();
        archive
            .by_name("config.ini")
            .unwrap()
            .read_to_string(&mut config)
            .unwrap();
        assert!(config.contains("numFade = 4\n"));
        assert!(config.contains("expTimeFirst = 30.0000\n"));
        for name in [
            "config.ini",
            "prusaslicer.ini",
            "resin_slicer_test00000.png",
        ] {
            let mut entry = archive.by_name(name).unwrap();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            assert!(!data.is_empty());
        }
        let _ = std::fs::remove_file(path);
    }
}
