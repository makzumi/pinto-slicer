//! Background export of a sliced job in the selected printer's native format.
//! Every format uses the profile's exact native LCD resolution; the optional
//! PNG masks are diagnostic copies of those same rasterized layers.
//!
//! A complete job is staged beside the destination first. Cancellation or an
//! encoder error only removes that staging directory: a previous valid job is
//! never truncated or deleted by a failed replacement.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crate::config::{JobFormat, PrintSettings, PrinterProfile};
use crate::formats;
use crate::slicer::SliceResult;

pub enum Msg {
    Progress { frac: f32, note: String },
    Done(String),
    Cancelled,
    Failed(String),
}

#[derive(Clone)]
pub struct ExportOptions {
    pub dir: PathBuf,
    pub file_stem: String,
    pub format: JobFormat,
    /// Also write one PNG per layer next to the job file.
    pub also_masks: bool,
}

pub fn start(
    options: ExportOptions,
    printer: PrinterProfile,
    settings: PrintSettings,
    slice: SliceResult,
    cancel: Arc<AtomicBool>,
) -> Receiver<Msg> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        if let Err(e) = run(&options, &printer, &settings, &slice, &tx, &cancel) {
            let _ = tx.send(Msg::Failed(e));
        }
    });
    rx
}

pub fn run_blocking(
    options: &ExportOptions,
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    on_progress: &mut dyn FnMut(f32, String),
) -> Result<String, String> {
    let (tx, rx) = channel();
    let mut last = String::new();
    let opts = options.clone();
    let printer = printer.clone();
    let settings = settings.clone();
    let slice = slice.clone();
    let handle = std::thread::spawn(move || {
        run(
            &opts,
            &printer,
            &settings,
            &slice,
            &tx,
            &AtomicBool::new(false),
        )
    });
    let mut result = Err("the export stopped without saying why".to_string());
    for msg in rx {
        match msg {
            Msg::Progress { frac, note } => on_progress(frac, note),
            Msg::Done(text) => {
                last = text.clone();
                result = Ok(text);
            }
            Msg::Cancelled => result = Err("the export was cancelled".into()),
            Msg::Failed(e) => result = Err(e),
        }
    }
    if handle.join().is_err() {
        return Err("the export thread crashed".into());
    }
    result.map(|t| if t.is_empty() { last } else { t })
}

static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);

/// A same-filesystem workspace. It is removed after success, cancellation or
/// a clean rollback, but retained when a rollback error leaves recovery files.
struct Staging {
    root: PathBuf,
    preserve_on_drop: bool,
}

impl Staging {
    fn new(parent: &Path) -> Result<Self, String> {
        for _ in 0..128 {
            let id = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
            let root = parent.join(format!(".pinto-slicer-stage-{}-{id}", std::process::id()));
            match std::fs::create_dir(&root) {
                Ok(()) => {
                    return Ok(Self {
                        root,
                        preserve_on_drop: false,
                    })
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("could not make an export workspace: {e}")),
            }
        }
        Err("could not reserve an export workspace".into())
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.preserve_on_drop {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

struct Replacement {
    destination: PathBuf,
    backup: PathBuf,
    had_previous: bool,
    installed: bool,
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Install all outputs, with the printer job last. Every replaced destination
/// is retained in the staging directory until the whole commit has succeeded.
fn commit_outputs(outputs: Vec<(PathBuf, PathBuf)>, staging: &mut Staging) -> Result<(), String> {
    let backups = staging.root.join("previous");
    std::fs::create_dir(&backups)
        .map_err(|e| format!("could not prepare the export commit: {e}"))?;
    let staged: Vec<PathBuf> = outputs.iter().map(|(source, _)| source.clone()).collect();
    let mut replacements: Vec<Replacement> = outputs
        .into_iter()
        .enumerate()
        .map(|(i, (_, destination))| Replacement {
            destination,
            backup: backups.join(i.to_string()),
            had_previous: false,
            installed: false,
        })
        .collect();

    let attempt = (|| -> std::io::Result<()> {
        for (source, replacement) in staged.iter().zip(replacements.iter_mut()) {
            if replacement.destination.exists() {
                std::fs::rename(&replacement.destination, &replacement.backup)?;
                replacement.had_previous = true;
            }
            std::fs::rename(source, &replacement.destination)?;
            replacement.installed = true;
        }
        Ok(())
    })();

    if let Err(error) = attempt {
        let mut rollback_errors = Vec::new();
        for replacement in replacements.iter().rev() {
            if replacement.installed {
                if let Err(e) = remove_path(&replacement.destination) {
                    rollback_errors.push(e.to_string());
                }
            }
            if replacement.had_previous {
                if let Err(e) = std::fs::rename(&replacement.backup, &replacement.destination) {
                    rollback_errors.push(e.to_string());
                }
            }
        }
        let rollback = if rollback_errors.is_empty() {
            "previous outputs were restored".to_string()
        } else {
            staging.preserve_on_drop = true;
            format!(
                "rollback also failed: {}; recovery files remain at {}",
                rollback_errors.join("; "),
                staging.root.display()
            )
        };
        return Err(format!("could not finish the export: {error}; {rollback}"));
    }
    Ok(())
}

fn file_stem(value: &str) -> Result<String, String> {
    let stem = if value.trim().is_empty() {
        "job".to_string()
    } else {
        value.trim().to_string()
    };
    let mut components = Path::new(&stem).components();
    let one_normal_name =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    if !one_normal_name || stem.contains(['/', '\\']) {
        return Err("the file name must not contain a folder or path".into());
    }
    Ok(stem)
}

fn cancelled(tx: &Sender<Msg>) -> Result<(), String> {
    let _ = tx.send(Msg::Cancelled);
    Ok(())
}

fn run(
    opts: &ExportOptions,
    printer: &PrinterProfile,
    settings: &PrintSettings,
    slice: &SliceResult,
    tx: &Sender<Msg>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    if slice.layers.is_empty() {
        return Err("nothing is sliced yet".into());
    }
    std::fs::create_dir_all(&opts.dir).map_err(|e| format!("could not make that folder: {e}"))?;
    let stem = file_stem(&opts.file_stem)?;
    let extension = opts.format.extension();
    let job_path = opts.dir.join(format!("{stem}.{extension}"));
    let mut staging = Staging::new(&opts.dir)?;
    let staged_job = staging.root.join(format!("{stem}.{extension}"));
    let res = printer.native_res();
    let mut done = 0usize;
    let _ = tx.send(Msg::Progress {
        frac: 0.0,
        note: format!("{} · {} × {} px", opts.format.label(), res.0, res.1),
    });
    let written = formats::write_job(
        opts.format,
        &staged_job,
        printer,
        settings,
        slice,
        res,
        &mut |i, all| {
            done = i;
            let _ = tx.send(Msg::Progress {
                frac: i as f32 / all as f32,
                note: format!("layer {i} of {all}"),
            });
            !cancel.load(Ordering::Relaxed)
        },
    )?;
    let Some(count) = written else {
        return cancelled(tx);
    };
    debug_assert_eq!(done, count);
    if cancel.load(Ordering::Relaxed) {
        return cancelled(tx);
    }

    let mut manifest = String::new();
    for (i, layer) in slice.layers.iter().enumerate() {
        manifest.push_str(&format!(
            "    {{\"layer\": {}, \"z_mm\": {:.3}}}{}\n",
            i + 1,
            layer.z,
            if i + 1 < slice.layers.len() { "," } else { "" }
        ));
    }
    let pixel = printer.pixel_mm();
    let json = format!(
        "{{\n  \"file\": \"{stem}.{extension}\",\n  \"format\": \"{}\",\n  \"printer\": \"{}\",\n  \"machine_name\": \"{}\",\n  \"build_volume_mm\": [{}, {}, {}],\n  \"pixel_size_mm\": [{:.8}, {:.8}],\n  \"resolution_pixels\": [{}, {}],\n  \"mirror\": [{}, {}],\n  \"layer_height_mm\": {},\n  \"layer_count\": {},\n  \"exposure_s\": {},\n  \"bottom_layers\": {},\n  \"bottom_exposure_s\": {},\n  \"supports\": {},\n  \"layers\": [\n{}  ]\n}}\n",
        opts.format.label(),
        printer.name,
        printer.machine_name,
        printer.build[0], printer.build[1], printer.build[2],
        pixel[0], pixel[1], res.0, res.1,
        printer.mirror[0], printer.mirror[1],
        slice.layer_height,
        slice.layers.len(),
        settings.exposure,
        settings.bottom_layers,
        settings.bottom_exposure,
        settings.supports.enabled,
        manifest
    );
    std::fs::write(staging.root.join("job.json"), json)
        .map_err(|e| format!("could not write job.json: {e}"))?;

    if opts.also_masks {
        let mask_dir = staging.root.join("masks");
        std::fs::create_dir_all(&mask_dir).map_err(|e| format!("could not make masks: {e}"))?;
        let mut buf: Vec<u8> = Vec::new();
        for (i, layer) in slice.layers.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return cancelled(tx);
            }
            formats::mask(layer, printer, res, &mut buf);
            let img = image::GrayImage::from_raw(res.0 as u32, res.1 as u32, buf.clone())
                .ok_or_else(|| "could not build a layer picture".to_string())?;
            let path = mask_dir.join(format!("layer_{:04}.png", i + 1));
            img.save(&path)
                .map_err(|e| format!("could not write {}: {e}", path.display()))?;
        }
    }

    let readme = format!(
        "Sliced job for {}\n==========================\n\n\
         {stem}.{extension} — {}.\n\
         job.json — the same settings in readable form.\n\
         {}\n\n\
         {} layers at {} mm, native mask {} × {} pixels.\n\
         Exposure {} s, bottom layers {} at {} s.\n\n\
         The writer follows the corresponding open-source format implementation.\n\
         Test a short print on the target machine before trusting a long job.\n",
        printer.name,
        opts.format.label(),
        if opts.also_masks {
            "masks/ — one diagnostic PNG per layer."
        } else {
            "No layer pictures were written."
        },
        slice.layers.len(),
        slice.layer_height,
        res.0,
        res.1,
        settings.exposure,
        settings.bottom_layers,
        settings.bottom_exposure,
    );
    std::fs::write(staging.root.join("README.txt"), readme)
        .map_err(|e| format!("could not write README.txt: {e}"))?;
    if cancel.load(Ordering::Relaxed) {
        return cancelled(tx);
    }

    // This is the point of no return. Once replacement starts, it finishes or
    // rolls back as one transaction; a late cancel cannot leave half a job.
    let mut outputs = vec![
        (staging.root.join("job.json"), opts.dir.join("job.json")),
        (staging.root.join("README.txt"), opts.dir.join("README.txt")),
    ];
    if opts.also_masks {
        outputs.push((staging.root.join("masks"), opts.dir.join("masks")));
    }
    outputs.push((staged_job, job_path.clone()));
    let _ = tx.send(Msg::Progress {
        frac: 1.0,
        note: "finalizing the job".into(),
    });
    commit_outputs(outputs, &mut staging)?;

    let _ = tx.send(Msg::Done(format!(
        "{} written ({} layers)",
        job_path.display(),
        slice.layers.len()
    )));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_printers;
    use crate::mesh;
    use crate::slicer;

    fn small_printer() -> PrinterProfile {
        let mut printer = default_printers()[0].clone();
        printer.resolution = [96, 64];
        printer
    }

    fn options(dir: &Path, masks: bool) -> ExportOptions {
        ExportOptions {
            dir: dir.to_path_buf(),
            file_stem: "part".into(),
            format: JobFormat::CxdlpV4,
            also_masks: masks,
        }
    }

    #[test]
    fn exporting_writes_the_native_job_and_description() {
        let sl = slicer::slice(&[mesh::sample_block()], 0.5);
        let dir = std::env::temp_dir().join("resin_slicer_export_test");
        let _ = std::fs::remove_dir_all(&dir);
        let (tx, _rx) = channel();
        run(
            &options(&dir, true),
            &small_printer(),
            &PrintSettings::default(),
            &sl,
            &tx,
            &AtomicBool::new(false),
        )
        .expect("export should work");
        assert!(dir.join("part.cxdlpv4").exists());
        assert!(dir.join("job.json").exists());
        assert!(dir.join("masks/layer_0001.png").exists());
        let json = std::fs::read_to_string(dir.join("job.json")).unwrap();
        assert!(json.contains("Creality CXDLP v4"));
        assert!(json.contains("\"resolution_pixels\": [96, 64]"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_slice_is_refused() {
        let dir = std::env::temp_dir().join("resin_slicer_export_empty");
        let (tx, _rx) = channel();
        let err = run(
            &options(&dir, false),
            &small_printer(),
            &PrintSettings::default(),
            &SliceResult::default(),
            &tx,
            &AtomicBool::new(false),
        );
        assert!(err.is_err());
    }

    #[test]
    fn path_like_file_stems_are_rejected_before_any_output_is_opened() {
        let sl = slicer::slice(&[mesh::sample_block()], 0.5);
        let root = std::env::temp_dir().join("resin_slicer_export_stem_safety");
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("nested");
        std::fs::create_dir_all(&dir).unwrap();
        let previous = root.join("outside.cxdlpv4");
        std::fs::write(&previous, b"do not truncate").unwrap();
        let absolute_stem = root.join("outside").to_string_lossy().to_string();
        for stem in [absolute_stem, "../outside".into(), "..\\outside".into()] {
            let mut opts = options(&dir, false);
            opts.file_stem = stem;
            let (tx, _rx) = channel();
            let error = run(
                &opts,
                &small_printer(),
                &PrintSettings::default(),
                &sl,
                &tx,
                &AtomicBool::new(true),
            )
            .expect_err("a path is not a file stem");
            assert!(error.contains("file name"), "{error}");
            assert_eq!(std::fs::read(&previous).unwrap(), b"do not truncate");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recovery_workspace_is_preserved_when_requested() {
        let dir = std::env::temp_dir().join("resin_slicer_export_recovery");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut staging = Staging::new(&dir).unwrap();
        let root = staging.root.clone();
        std::fs::write(root.join("previous-job"), b"recovery").unwrap();
        staging.preserve_on_drop = true;
        drop(staging);
        assert_eq!(
            std::fs::read(root.join("previous-job")).unwrap(),
            b"recovery"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancelled_export_throws_away_what_it_wrote() {
        let sl = slicer::slice(&[mesh::sample_block()], 0.25);
        let dir = std::env::temp_dir().join("resin_slicer_export_cancelled");
        let _ = std::fs::remove_dir_all(&dir);
        let (tx, rx) = channel();
        run(
            &options(&dir, true),
            &small_printer(),
            &PrintSettings::default(),
            &sl,
            &tx,
            &AtomicBool::new(true),
        )
        .expect("stopping is not a failure");
        assert!(!dir.join("part.cxdlpv4").exists());
        assert!(!dir.join("job.json").exists());
        let said: Vec<Msg> = rx.try_iter().collect();
        assert!(matches!(said.last(), Some(Msg::Cancelled)));
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".pinto-slicer-stage-")
            })
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_a_replacement_preserves_the_previous_job() {
        let sl = slicer::slice(&[mesh::sample_block()], 0.5);
        let dir = std::env::temp_dir().join("resin_slicer_export_preserve_cancel");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("masks")).unwrap();
        std::fs::write(dir.join("part.cxdlpv4"), b"previous valid job").unwrap();
        std::fs::write(dir.join("job.json"), b"previous manifest").unwrap();
        std::fs::write(dir.join("README.txt"), b"previous readme").unwrap();
        std::fs::write(dir.join("masks/layer_0001.png"), b"previous mask").unwrap();
        let (tx, _rx) = channel();
        run(
            &options(&dir, true),
            &small_printer(),
            &PrintSettings::default(),
            &sl,
            &tx,
            &AtomicBool::new(true),
        )
        .expect("stopping is not a failure");
        assert_eq!(
            std::fs::read(dir.join("part.cxdlpv4")).unwrap(),
            b"previous valid job"
        );
        assert_eq!(
            std::fs::read(dir.join("job.json")).unwrap(),
            b"previous manifest"
        );
        assert_eq!(
            std::fs::read(dir.join("README.txt")).unwrap(),
            b"previous readme"
        );
        assert_eq!(
            std::fs::read(dir.join("masks/layer_0001.png")).unwrap(),
            b"previous mask"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_encoder_error_preserves_the_previous_job() {
        let sl = slicer::slice(&[mesh::sample_block()], 0.5);
        let dir = std::env::temp_dir().join("resin_slicer_export_preserve_error");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("part.cxdlpv4"), b"previous valid job").unwrap();
        std::fs::write(dir.join("job.json"), b"previous manifest").unwrap();
        let mut printer = small_printer();
        // The X1 pixel-pair codec requires an even row width, so this fails
        // only after the staged output has been opened.
        printer.resolution[0] = 95;
        let (tx, _rx) = channel();
        assert!(run(
            &options(&dir, false),
            &printer,
            &PrintSettings::default(),
            &sl,
            &tx,
            &AtomicBool::new(false),
        )
        .is_err());
        assert_eq!(
            std::fs::read(dir.join("part.cxdlpv4")).unwrap(),
            b"previous valid job"
        );
        assert_eq!(
            std::fs::read(dir.join("job.json")).unwrap(),
            b"previous manifest"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
