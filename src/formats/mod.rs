//! Printer-job writers. Every encoder consumes the same vector slice and
//! rasterizes at the selected profile's exact native panel resolution.

mod anycubic;
mod ctb_goo;
mod sl1;

use std::path::Path;

use crate::config::{JobFormat, PrintSettings, PrinterProfile};
use crate::cxdlp;
use crate::slicer::{self, SliceResult};

pub type Progress<'a> = &'a mut dyn FnMut(usize, usize) -> bool;

pub fn write_job(
    format: JobFormat,
    path: &Path,
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    res: (usize, usize),
    progress: Progress<'_>,
) -> Result<Option<usize>, String> {
    if slice.layers.is_empty() {
        return Err("nothing is sliced yet".into());
    }
    if res.0 == 0 || res.1 == 0 || res.0 > u16::MAX as usize || res.1 > u16::MAX as usize {
        return Err(format!(
            "unsupported layer resolution {} × {}",
            res.0, res.1
        ));
    }
    match format {
        JobFormat::CxdlpV4 => cxdlp::write_v4(path, printer, settings, slice, res, progress),
        JobFormat::Cxdlp => cxdlp::write_v3(path, printer, settings, slice, res, progress),
        JobFormat::Ctb | JobFormat::Goo => {
            ctb_goo::write_job(format, path, printer, settings, slice, res, progress)
        }
        JobFormat::Sl1 | JobFormat::Sl1s => {
            sl1::write_job(format, path, printer, settings, slice, res, progress)
        }
        JobFormat::Photon
        | JobFormat::Pwmo
        | JobFormat::Pwmx
        | JobFormat::Pm3
        | JobFormat::Pws
        | JobFormat::Pwma => {
            anycubic::write_job(format, path, printer, settings, slice, res, progress)
        }
    }
}

/// Rasterize one vector layer and apply the printer's optical mirroring.
pub(crate) fn mask(
    layer: &slicer::Layer,
    printer: &PrinterProfile,
    res: (usize, usize),
    buf: &mut Vec<u8>,
) {
    let min = [-printer.build[0] * 0.5, -printer.build[1] * 0.5];
    let max = [printer.build[0] * 0.5, printer.build[1] * 0.5];
    slicer::fill_mask(layer, min, max, res.0, res.1, buf);
    if printer.mirror[0] {
        for row in buf.chunks_exact_mut(res.0) {
            row.reverse();
        }
    }
    if printer.mirror[1] {
        for y in 0..res.1 / 2 {
            let other = res.1 - 1 - y;
            for x in 0..res.0 {
                buf.swap(y * res.0 + x, other * res.0 + x);
            }
        }
    }
}

pub(crate) fn io(path: &Path, e: impl std::fmt::Display) -> String {
    format!("could not write {}: {e}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::export_printers;
    use crate::mesh;

    #[test]
    fn every_requested_format_writes_a_complete_job() {
        let slice = crate::slicer::slice(&[mesh::sample_block()], 0.5);
        let dir = std::env::temp_dir().join("resin_slicer_all_formats");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for format in JobFormat::ALL {
            let mut printer = export_printers()
                .into_iter()
                .find(|printer| printer.supports_format(format))
                .unwrap_or_else(|| panic!("no profile accepts {format:?}"));
            printer.resolution = [96, 64];
            let path = dir.join(format!("test.{}", format.extension()));
            let written = write_job(
                format,
                &path,
                &printer,
                &PrintSettings::default(),
                &slice,
                (96, 64),
                &mut |_, _| true,
            )
            .unwrap_or_else(|e| panic!("{format:?}: {e}"));
            assert_eq!(written, Some(slice.layers.len()), "{format:?}");
            let bytes = std::fs::read(&path).unwrap();
            assert!(bytes.len() > 100, "{format:?} wrote an empty job");
            match format {
                JobFormat::Cxdlp | JobFormat::CxdlpV4 => assert_eq!(&bytes[4..13], b"CXSW3DV2\0"),
                JobFormat::Photon => assert_eq!(&bytes[..4], &[0x19, 0, 0xfd, 0x12]),
                JobFormat::Pwmo
                | JobFormat::Pwmx
                | JobFormat::Pm3
                | JobFormat::Pws
                | JobFormat::Pwma => {
                    assert_eq!(&bytes[..8], b"ANYCUBIC")
                }
                JobFormat::Sl1 | JobFormat::Sl1s => assert_eq!(&bytes[..2], b"PK"),
                JobFormat::Ctb | JobFormat::Goo => {}
            }
        }
        if std::env::var_os("KEEP_FORMAT_SAMPLES").is_some() {
            eprintln!("format samples kept in {}", dir.display());
        } else {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
