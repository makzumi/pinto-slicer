//! `--export <job.ext> <model ... | folder>`: slice without opening a window.
//! The selected printer determines both the exact LCD resolution and format.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::{export_printers, JobFormat, PrintSettings, PrinterProfile};
use crate::export::{self, ExportOptions};
use crate::layout;
use crate::mesh;
use crate::slicer;
use crate::supports;

fn models_in(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if ext == "stl" || ext == "obj" {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn choose_printer(name: Option<&str>, out: &Path) -> Result<(PrinterProfile, JobFormat), String> {
    let printers = export_printers();
    let ext = out
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let requested_format = if ext.is_empty() {
        None
    } else {
        JobFormat::from_extension(&ext)
    };
    if let Some(name) = name {
        let query = name.trim().to_ascii_lowercase();
        let printer = printers
            .into_iter()
            .find(|p| {
                p.name.to_ascii_lowercase() == query
                    || p.machine_name.to_ascii_lowercase() == query
                    || p.name.to_ascii_lowercase().contains(&query)
            })
            .ok_or_else(|| {
                format!(
                    "unknown printer '{name}'. Available printers:\n  {}",
                    export_printers()
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>()
                        .join("\n  ")
                )
            })?;
        let format = requested_format.unwrap_or(printer.format);
        return Ok((printer, format));
    }
    if let Some(format) = requested_format {
        if let Some(profile) = printers.iter().find(|p| p.supports_format(format)) {
            return Ok((profile.clone(), format));
        }
    }
    Ok((printers[0].clone(), printers[0].format))
}

pub fn run(
    sources: &[PathBuf],
    out: &Path,
    also_masks: bool,
    printer_name: Option<&str>,
) -> Result<(), String> {
    let mut files: Vec<PathBuf> = Vec::new();
    for s in sources {
        if s.is_dir() {
            files.extend(models_in(s));
        } else {
            files.push(s.clone());
        }
    }
    if files.is_empty() {
        return Err("no STL or OBJ files to slice".into());
    }

    let (printer, format) = choose_printer(printer_name, out)?;
    if !printer.supports_format(format) {
        return Err(format!(
            "{} does not accept .{} files",
            printer.name,
            format.extension()
        ));
    }
    if let Some(ext) = out.extension().and_then(|s| s.to_str()) {
        if !ext.eq_ignore_ascii_case(format.extension()) {
            return Err(format!(
                "{} writes .{}, not .{}; choose a matching printer or output extension",
                printer.name,
                format.extension(),
                ext
            ));
        }
    }
    println!(
        "{} · {} · {} × {} px",
        printer.name,
        format.label(),
        printer.resolution[0],
        printer.resolution[1]
    );
    let settings = PrintSettings::default();
    let t = Instant::now();
    let mut pieces: Vec<mesh::Mesh> = Vec::new();
    for f in &files {
        pieces.push(mesh::load_model(f)?);
    }
    println!(
        "loaded {} pieces in {} ms",
        pieces.len(),
        t.elapsed().as_millis()
    );

    if pieces.len() > 1 {
        let bounds: Vec<_> = pieces.iter().map(|m| m.bounds()).collect();
        for slot in layout::arrange(&bounds, printer.build, 6.0) {
            pieces[slot.index].translate(slot.delta);
        }
    }

    let t = Instant::now();
    supports::lift(&mut pieces, settings.supports.height_from_platform);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let model = supports::generate(&pieces, &settings.supports, &cancel);
    let support_mesh = model.mesh(12);
    let mut all = pieces.clone();
    all.push(support_mesh.clone());
    let slice = slicer::slice(&all, settings.layer_height);
    println!(
        "{} tips, {} layers in {} ms",
        model.stats.tips,
        slice.layers.len(),
        t.elapsed().as_millis()
    );
    println!("{}", model.report());

    let dir = out
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = out
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "job".into());
    let actual_out = dir.join(format!("{stem}.{}", format.extension()));
    let options = ExportOptions {
        dir,
        file_stem: stem,
        format,
        also_masks,
    };
    let t = Instant::now();
    let mut next_marks = 10;
    let done = export::run_blocking(&options, &printer, &settings, &slice, &mut |frac, note| {
        if (frac * 100.0) as usize >= next_marks {
            println!("  {:.0}% ({note})", frac * 100.0);
            next_marks += 20;
        }
    })?;
    let secs = t.elapsed().as_secs_f32();
    let size = std::fs::metadata(&actual_out).map(|m| m.len()).unwrap_or(0);
    let res = printer.native_res();
    println!(
        "{done}\n{:.1} s, {:.1} MB at {}x{} pixels a layer",
        secs,
        size as f32 / 1_048_576.0,
        res.0,
        res.1
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_output_extension_selects_a_compatible_profile_alternative() {
        let (printer, format) = choose_printer(None, Path::new("job.sl1s")).unwrap();
        assert_eq!(format, JobFormat::Sl1s);
        assert!(printer.name.contains("Prusa"));
        let (printer, format) = choose_printer(None, Path::new("job.pws")).unwrap();
        assert_eq!(format, JobFormat::Pws);
        assert!(printer.name.contains("Photon"));
        let (_, format) = choose_printer(Some("HALOT-X1"), Path::new("job.cxdlpv4")).unwrap();
        assert_eq!(format, JobFormat::CxdlpV4);
        let (printer, format) = choose_printer(None, Path::new("job.pwma")).unwrap();
        assert_eq!(format, JobFormat::Pwma);
        assert_eq!(printer.native_res(), (3840, 2400));
    }
}
