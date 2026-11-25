//! CTB and GOO output through the audited `msla_format` encoders.
//!
//! Keeping these two codecs in a maintained crate avoids another local copy of
//! CTB's encryption/page table and GOO's differential RLE implementation.

use std::path::Path;

use msla_format::serde::DynamicSerializer;
use msla_format::slice::{SliceConfig, SliceResult as MslaSlice};
use msla_format::units::{Milimeters, MilimetersPerMinute, Minutes, Seconds};
use msla_format::EncodableLayer;
use msla_format::SlicedFile;

use crate::config::{JobFormat, PrintSettings, PrinterProfile};
use crate::slicer::SliceResult;

use super::{mask, Progress};

fn config(
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    res: (usize, usize),
) -> SliceConfig {
    let mut c = SliceConfig::default();
    c.platform_resolution.x = res.0 as u32;
    c.platform_resolution.y = res.1 as u32;
    c.platform_size.x = Milimeters::new(printer.build[0]);
    c.platform_size.y = Milimeters::new(printer.build[1]);
    c.platform_size.z = Milimeters::new(printer.build[2]);
    c.slice_height = Milimeters::new(slice.layer_height);
    c.first_layers = settings.bottom_layers;
    c.transition_layers = 0;
    c.exposure_config.exposure_time = Seconds::new(settings.exposure);
    c.first_exposure_config.exposure_time = Seconds::new(settings.bottom_exposure);
    // One lift/retract pair, sized and timed by the user's motion settings.
    let rise = Milimeters::new(settings.rising_height.max(0.0));
    let speed = (Milimeters::new(settings.motor_speed.max(1.0)) / Minutes::new(1.0)).convert();
    c.exposure_config.lift_distance = rise;
    c.exposure_config.retract_distance = rise;
    c.first_exposure_config.lift_distance = rise;
    c.first_exposure_config.retract_distance = rise;
    for exposure in [&mut c.exposure_config, &mut c.first_exposure_config] {
        exposure.lift_speed = speed;
        exposure.retract_speed = speed;
    }
    c
}

fn runs(mask: &[u8], mut add: impl FnMut(u64, u8)) {
    let mut at = 0;
    while at < mask.len() {
        let value = mask[at];
        let mut end = at + 1;
        while end < mask.len() && mask[end] == value {
            end += 1;
        }
        add((end - at) as u64, value);
        at = end;
    }
}

fn preview_rgba(mask: &[u8], res: (usize, usize)) -> image::RgbaImage {
    let gray = image::GrayImage::from_raw(res.0 as u32, res.1 as u32, mask.to_vec())
        .unwrap_or_else(|| image::GrayImage::new(res.0 as u32, res.1 as u32));
    let gray = image::imageops::resize(&gray, 400, 300, image::imageops::FilterType::Triangle);
    image::RgbaImage::from_fn(400, 300, |x, y| {
        let v = gray.get_pixel(x, y)[0];
        image::Rgba([v, v, v, 255])
    })
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
    let config = config(printer, settings, slice, res);
    let total = slice.layers.len();
    let mut buf = Vec::new();
    match format {
        JobFormat::Ctb => {
            let mut layers = Vec::with_capacity(total);
            for (i, layer) in slice.layers.iter().enumerate() {
                mask(layer, printer, res, &mut buf);
                let mut encoder = msla_format::ctb::LayerEncoder::default();
                runs(&buf, |n, value| encoder.add_run(n, value));
                layers.push(encoder.finish(i as u32, &config));
                if !progress(i + 1, total) {
                    return Ok(None);
                }
            }
            let mut file = msla_format::ctb::File::from_slice_result(MslaSlice {
                layers,
                voxels: 0,
                slice_config: &config,
            });
            file.machine_name = printer.machine_name.clone();
            file.resin_parameters.machine_name = printer.machine_name.clone();
            file.print_time = (settings.estimated_minutes(total) * 60.0).round() as u32;
            let off = Seconds::new(settings.light_off_delay.max(0.0));
            file.light_off_delay = off;
            file.bottom_light_off_delay = off;
            // Disable msla_format's illustrative second-stage movements: this
            // writer deliberately emits one simple lift/retract pair.
            file.bottom_lift_height_2 = Milimeters::new(0.0);
            file.lift_height_2 = Milimeters::new(0.0);
            file.retract_height_2 = Milimeters::new(0.0);
            file.bottom_retract_height_2 = Milimeters::new(0.0);
            file.bottom_lift_speed_2 = MilimetersPerMinute::new(0.0);
            file.lift_speed_2 = MilimetersPerMinute::new(0.0);
            file.retract_speed_2 = MilimetersPerMinute::new(0.0);
            file.bottom_retract_speed_2 = MilimetersPerMinute::new(0.0);
            file.rest_time_after_lift = Seconds::new(0.0);
            file.rest_time_after_retract = Seconds::new(0.0);
            file.rest_time_after_retract_2 = Seconds::new(0.0);
            file.rest_time_after_lift_2 = Seconds::new(0.0);
            file.rest_time_after_lift_3 = Seconds::new(0.0);
            file.rest_time_before_lift = Seconds::new(0.0);
            for layer in &mut file.layers {
                layer.light_off_delay = off;
                layer.lift_height_2 = Milimeters::new(0.0);
                layer.lift_speed_2 = MilimetersPerMinute::new(0.0);
                layer.retract_height_2 = Milimeters::new(0.0);
                layer.retract_speed_2 = MilimetersPerMinute::new(0.0);
                layer.rest_time_before_lift = Seconds::new(0.0);
                layer.rest_time_after_lift = Seconds::new(0.0);
                layer.rest_time_after_retract = Seconds::new(0.0);
            }
            SlicedFile::set_preview(&mut file, &preview_rgba(&buf, res));
            let mut ser = DynamicSerializer::new();
            file.serialize(&mut ser);
            std::fs::write(path, ser.into_inner()).map_err(|e| super::io(path, e))?;
        }
        JobFormat::Goo => {
            let mut layers = Vec::with_capacity(total);
            for (i, layer) in slice.layers.iter().enumerate() {
                mask(layer, printer, res, &mut buf);
                let mut encoder = msla_format::goo::LayerEncoder::new();
                runs(&buf, |n, value| encoder.add_run(n, value));
                layers.push(encoder.finish(i as u32, &config));
                if !progress(i + 1, total) {
                    return Ok(None);
                }
            }
            let mut file = msla_format::goo::File::from_slice_result(MslaSlice {
                layers,
                voxels: 0,
                slice_config: &config,
            });
            file.header.printer_name =
                msla_format::serde::SizedString::new(printer.machine_name.as_bytes());
            let off = Seconds::new(settings.light_off_delay.max(0.0));
            file.header.exposure_delay_mode = msla_format::goo::ExposureDelayMode::TurnOffTime;
            file.header.turn_off_time = off;
            file.header.bottom_after_retract_time = Seconds::new(0.0);
            file.header.after_retract_time = Seconds::new(0.0);
            file.header.printing_time = (settings.estimated_minutes(total) * 60.0).round() as u32;
            for layer in &mut file.layers {
                layer.layer_off_time = off;
                layer.after_retract_time = Seconds::new(0.0);
            }
            SlicedFile::set_preview(&mut file, &preview_rgba(&buf, res));
            let mut ser = DynamicSerializer::new();
            file.serialize(&mut ser);
            std::fs::write(path, ser.into_inner()).map_err(|e| super::io(path, e))?;
        }
        _ => unreachable!(),
    }
    Ok(Some(total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_printers;
    use crate::mesh;

    #[test]
    fn ctb_and_goo_have_real_signatures_and_read_back() {
        let sl = crate::slicer::slice(&[mesh::sample_block()], 0.5);
        for (format, printer) in [
            (JobFormat::Ctb, default_printers()[2].clone()),
            (JobFormat::Goo, default_printers()[3].clone()),
        ] {
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
            let mut des = msla_format::serde::SliceDeserializer::new(&bytes);
            match format {
                JobFormat::Ctb => {
                    let file = msla_format::ctb::File::deserialize(&mut des).unwrap();
                    assert_eq!(file.layers.len(), sl.layers.len());
                    assert_eq!((file.resolution.x, file.resolution.y), (96, 64));
                    assert_eq!(file.lift_speed.raw(), 250.0);
                    assert_eq!(file.retract_speed.raw(), 250.0);
                    assert_eq!(file.light_off_delay.raw(), 4.0);
                    assert_eq!(file.layers[0].light_off_delay.raw(), 4.0);
                    assert_eq!(file.bottom_lift_height_2.raw(), 0.0);
                }
                JobFormat::Goo => {
                    let file = msla_format::goo::File::deserialize(&mut des).unwrap();
                    assert_eq!(file.layers.len(), sl.layers.len());
                    assert_eq!(
                        (file.header.x_resolution, file.header.y_resolution),
                        (96, 64)
                    );
                    assert_eq!(file.header.lift_speed.raw(), 250.0);
                    assert_eq!(file.header.retract_speed.raw(), 250.0);
                    assert_eq!(file.header.turn_off_time.raw(), 4.0);
                    assert_eq!(file.layers[0].layer_off_time.raw(), 4.0);
                }
                _ => unreachable!(),
            }
            let _ = std::fs::remove_file(path);
        }
    }
}
