//! Self-contained project files and the shared undo/redo history.
//!
//! Meshes are reference counted inside snapshots: restoring or remembering a
//! project does not duplicate its vertex and triangle arrays. Saving a project
//! serializes those arrays into the file, so a `.rslice` never needs the STL
//! or OBJ it was originally made from.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bincode::Options;
use serde::{Deserialize, Serialize};

use crate::config::{JobFormat, PrintSettings, PrinterProfile};
use crate::mesh::{Mesh, Transform};

const MAGIC: &[u8; 8] = b"RSLICE\0\0";
const VERSION: u16 = 1;
const MAX_PROJECT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const HISTORY_LIMIT: usize = 40;
const COALESCE_FOR: Duration = Duration::from_millis(350);
const COORDINATE_LIMIT: f32 = 1_000_000.0;

fn finite(value: f32) -> bool {
    value.is_finite() && value.abs() <= COORDINATE_LIMIT
}

fn finite_vec(value: crate::mesh::Vec3) -> bool {
    finite(value.x) && finite(value.y) && finite(value.z)
}

fn positive(value: f32) -> bool {
    finite(value) && value > 0.0
}

fn nonnegative(value: f32) -> bool {
    finite(value) && value >= 0.0
}

fn valid_transform(transform: Transform) -> bool {
    finite_vec(transform.pos)
        && finite_vec(transform.rot_deg)
        && positive(transform.scale.x)
        && positive(transform.scale.y)
        && positive(transform.scale.z)
        && transform.scale.x <= 1000.0
        && transform.scale.y <= 1000.0
        && transform.scale.z <= 1000.0
}

fn valid_supports(config: &crate::supports::SupportConfig) -> bool {
    let positive_values = [
        config.touch_tip_distance,
        config.tip.upper_diameter,
        config.tip.lower_diameter,
        config.tip.length,
        config.tip.contact_diameter,
        config.branch_diameter,
        config.trunk_diameter,
        config.trunk_diameter_max,
        config.foot_diameter,
    ];
    let nonnegative_values = [
        config.height_from_platform,
        config.tip.contact_depth,
        config.base.plate_thickness,
        config.base.margin,
    ];
    let angles = [
        config.overhang_angle_deg,
        config.branch_angle_deg,
        config.base.chamfer_deg,
    ];
    positive_values.into_iter().all(positive)
        && nonnegative_values.into_iter().all(nonnegative)
        && angles
            .into_iter()
            .all(|angle| finite(angle) && angle.abs() <= 360.0)
}

fn valid_settings(settings: &PrintSettings) -> bool {
    positive(settings.layer_height)
        && nonnegative(settings.exposure)
        && nonnegative(settings.bottom_exposure)
        && nonnegative(settings.layer_move_s)
        && settings.exposure <= 86_400.0
        && settings.bottom_exposure <= 86_400.0
        && settings.layer_move_s <= 86_400.0
        && settings.bottom_layers <= 20_000
        && valid_supports(&settings.supports)
}

/// Recognize a project by content as well as by its conventional extension.
pub fn is_project(path: &Path) -> bool {
    std::fs::File::open(path)
        .and_then(|mut file| {
            let mut magic = [0_u8; 8];
            file.read_exact(&mut magic).map(|_| &magic == MAGIC)
        })
        .unwrap_or(false)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectModel {
    pub mesh: Arc<Mesh>,
    pub transform: Transform,
}

/// Every user-owned input that makes up a project. Derived support meshes,
/// slice rasters, pick trees and GPU textures are deliberately not included.
#[derive(Clone, Serialize, Deserialize)]
pub struct Project {
    pub printers: Vec<PrinterProfile>,
    pub printer_idx: usize,
    pub settings: PrintSettings,
    pub models: Vec<ProjectModel>,
    pub selection: Vec<usize>,
    pub export_dir: String,
    pub export_stem: String,
    pub export_format: Option<JobFormat>,
    pub export_masks: bool,
    pub layout_gap: f32,
    pub snap: bool,
    pub snap_deg: f32,
}

impl Project {
    fn validate(&self) -> Result<(), String> {
        if self.printers.is_empty() {
            return Err("the project has no printer profiles".into());
        }
        if self.printer_idx >= self.printers.len() {
            return Err("the project's selected printer is out of range".into());
        }
        for printer in &self.printers {
            if printer.resolution.contains(&0) || printer.build.iter().any(|v| !positive(*v)) {
                return Err(format!("printer '{}' has invalid geometry", printer.name));
            }
            if !printer.supports_format(printer.format) {
                return Err(format!(
                    "printer '{}' has an invalid native format",
                    printer.name
                ));
            }
        }
        for (i, model) in self.models.iter().enumerate() {
            if !valid_transform(model.transform) || model.mesh.verts.iter().any(|&v| !finite_vec(v))
            {
                return Err(format!("model {i} has non-finite or unreasonable geometry"));
            }
            if model
                .mesh
                .tris
                .iter()
                .flatten()
                .any(|&index| index as usize >= model.mesh.verts.len())
            {
                return Err(format!("model {i} has a triangle outside its vertex array"));
            }
        }
        if !valid_settings(&self.settings) {
            return Err("the project has invalid print or support settings".into());
        }
        if !nonnegative(self.layout_gap) || !positive(self.snap_deg) || self.snap_deg > 360.0 {
            return Err("the project has invalid layout or snapping settings".into());
        }
        let printer = &self.printers[self.printer_idx];
        if self
            .export_format
            .is_some_and(|format| !printer.supports_format(format))
        {
            return Err("the project's export format is incompatible with its printer".into());
        }
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            let backup = sidecar(path, "backup");
            if backup.exists() {
                std::fs::rename(&backup, path).map_err(|e| {
                    format!(
                        "could not recover the previous project from {}: {e}",
                        backup.display()
                    )
                })?;
            }
        }
        let file = std::fs::File::open(path)
            .map_err(|e| format!("could not open {}: {e}", path.display()))?;
        let mut reader = std::io::BufReader::new(file);
        let mut magic = [0_u8; 8];
        reader
            .read_exact(&mut magic)
            .map_err(|_| "that is not a Pinto Slicer project".to_string())?;
        if &magic != MAGIC {
            return Err("that is not a Pinto Slicer project".into());
        }
        let mut version = [0_u8; 2];
        reader
            .read_exact(&mut version)
            .map_err(|_| "the project version is missing".to_string())?;
        let version = u16::from_le_bytes(version);
        if version != VERSION {
            return Err(format!(
                "project version {version} is not supported (this app reads version {VERSION})"
            ));
        }
        let project: Self = bincode::options()
            .with_fixint_encoding()
            .with_limit(MAX_PROJECT_BYTES)
            .deserialize_from(&mut reader)
            .map_err(|e| format!("could not decode the project: {e}"))?;
        let mut trailing = [0_u8; 1];
        if reader.read(&mut trailing).map_err(|e| e.to_string())? != 0 {
            return Err("the project has unexpected data after its end".into());
        }
        project.validate()?;
        Ok(project)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        let temporary = sidecar(path, "save-tmp");
        let backup = sidecar(path, "backup");
        let write = (|| -> Result<(), String> {
            let file = std::fs::File::create(&temporary)
                .map_err(|e| format!("could not create a project temporary file: {e}"))?;
            let mut writer = std::io::BufWriter::new(file);
            writer.write_all(MAGIC).map_err(|e| e.to_string())?;
            writer
                .write_all(&VERSION.to_le_bytes())
                .map_err(|e| e.to_string())?;
            bincode::options()
                .with_fixint_encoding()
                .with_limit(MAX_PROJECT_BYTES)
                .serialize_into(&mut writer, self)
                .map_err(|e| format!("could not encode the project: {e}"))?;
            writer.flush().map_err(|e| e.to_string())?;
            writer
                .get_ref()
                .sync_all()
                .map_err(|e| format!("could not finish the project write: {e}"))?;
            Ok(())
        })();
        if let Err(error) = write {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }

        let had_previous = path.exists();
        // On Unix this is the atomic replace: readers see either the old or
        // new complete file, never a missing destination. Platforms that
        // refuse replacement fall back to the recoverable two-step below.
        if std::fs::rename(&temporary, path).is_ok() {
            let _ = std::fs::remove_file(&backup);
            sync_parent(path);
            return Ok(());
        }
        if !had_previous {
            let _ = std::fs::remove_file(&temporary);
            return Err("could not install the project".into());
        }

        let _ = std::fs::remove_file(&backup);
        std::fs::rename(path, &backup).map_err(|e| {
            let _ = std::fs::remove_file(&temporary);
            format!("could not preserve the previous project: {e}")
        })?;
        if let Err(error) = std::fs::rename(&temporary, path) {
            let restoration = if had_previous {
                std::fs::rename(&backup, path)
                    .map(|_| "previous project restored".to_string())
                    .unwrap_or_else(|restore| {
                        format!(
                            "previous project remains at {} ({restore})",
                            backup.display()
                        )
                    })
            } else {
                "no previous project was changed".to_string()
            };
            let _ = std::fs::remove_file(&temporary);
            return Err(format!(
                "could not install the project: {error}; {restoration}"
            ));
        }
        if had_previous {
            let _ = std::fs::remove_file(backup);
        }
        sync_parent(path);
        Ok(())
    }
}

fn sidecar(path: &Path, kind: &str) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project.rslice");
    if kind == "backup" {
        path.with_file_name(format!(".{name}.backup"))
    } else {
        path.with_file_name(format!(".{name}.{}.{}", std::process::id(), kind))
    }
}

#[cfg(unix)]
fn sync_parent(path: &Path) {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        let _ = std::fs::File::open(parent).and_then(|dir| dir.sync_all());
    }
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) {}

pub enum IoResult {
    Saved(PathBuf),
    Loaded(PathBuf, Box<Project>),
}

/// After-state history. A change is marked immediately and captured after a
/// short quiet period, so a slider or a file load does not create dozens of
/// tiny undo steps. Undo/redo always flushes the current edit first.
pub struct History {
    states: Vec<Project>,
    index: usize,
    pending: bool,
    last_change: Option<Instant>,
}

impl History {
    pub fn new(initial: Project) -> Self {
        Self {
            states: vec![initial],
            index: 0,
            pending: false,
            last_change: None,
        }
    }

    pub fn mark_changed(&mut self) {
        self.pending = true;
        self.last_change = Some(Instant::now());
    }

    pub fn tick(&mut self, current: &Project) {
        if self.pending
            && self
                .last_change
                .is_some_and(|at| at.elapsed() >= COALESCE_FOR)
        {
            self.flush(current.clone());
        }
    }

    pub fn flush(&mut self, current: Project) {
        if !self.pending {
            return;
        }
        self.states.truncate(self.index + 1);
        self.states.push(current);
        if self.states.len() > HISTORY_LIMIT {
            self.states.remove(0);
        }
        self.index = self.states.len() - 1;
        self.pending = false;
        self.last_change = None;
    }

    pub fn can_undo(&self) -> bool {
        self.index > 0 || self.pending
    }

    pub fn can_redo(&self) -> bool {
        !self.pending && self.index + 1 < self.states.len()
    }

    pub fn undo(&mut self, current: &Project) -> Option<Project> {
        self.flush(current.clone());
        if self.index == 0 {
            return None;
        }
        self.index -= 1;
        Some(self.states[self.index].clone())
    }

    pub fn redo(&mut self, current: &Project) -> Option<Project> {
        self.flush(current.clone());
        if self.index + 1 >= self.states.len() {
            return None;
        }
        self.index += 1;
        Some(self.states[self.index].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_printers;
    use crate::mesh::{self, Vec3};

    fn sample() -> Project {
        Project {
            printers: default_printers(),
            printer_idx: 0,
            settings: PrintSettings::default(),
            models: vec![ProjectModel {
                mesh: Arc::new(mesh::sample_block()),
                transform: Transform::default(),
            }],
            selection: vec![0],
            export_dir: "/tmp".into(),
            export_stem: "case".into(),
            export_format: None,
            export_masks: true,
            layout_gap: 6.0,
            snap: true,
            snap_deg: 5.0,
        }
    }

    #[test]
    fn project_round_trip_embeds_the_mesh_and_settings() {
        let mut project = sample();
        project.models[0].transform.pos = Vec3::new(12.0, -4.0, 6.0);
        project.settings.exposure = 3.25;
        let path = std::env::temp_dir().join("resin_slicer_project_roundtrip.rslice");
        project.save(&path).unwrap();
        let loaded = Project::read(&path).unwrap();
        assert_eq!(loaded.models.len(), 1);
        assert_eq!(loaded.models[0].mesh.verts, project.models[0].mesh.verts);
        assert_eq!(loaded.models[0].mesh.tris, project.models[0].mesh.tris);
        assert_eq!(loaded.models[0].mesh.name, "calibration block");
        assert_eq!(loaded.models[0].transform.pos.x, 12.0);
        assert_eq!(loaded.settings.exposure, 3.25);
        assert_eq!(loaded.export_stem, "case");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_invalid_project_is_rejected() {
        let path = std::env::temp_dir().join("resin_slicer_project_invalid.rslice");
        std::fs::write(&path, b"not a project").unwrap();
        assert!(Project::read(&path).is_err());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_geometry_cannot_replace_a_valid_project() {
        let mut project = sample();
        let path = std::env::temp_dir().join("resin_slicer_project_validation.rslice");
        project.save(&path).unwrap();
        assert!(is_project(&path));
        Arc::make_mut(&mut project.models[0].mesh).verts[0].x = f32::NAN;
        assert!(project.save(&path).is_err());
        let previous = Project::read(&path).unwrap();
        assert!(previous.models[0].mesh.verts[0].x.is_finite());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn undo_redo_and_a_new_branch_have_normal_history_semantics() {
        let base = sample();
        let mut changed = base.clone();
        changed.models[0].transform.pos.x = 20.0;
        let mut history = History::new(base.clone());
        history.mark_changed();
        assert!(history.can_undo());
        let undone = history.undo(&changed).unwrap();
        assert_eq!(undone.models[0].transform.pos.x, 0.0);
        let redone = history.redo(&undone).unwrap();
        assert_eq!(redone.models[0].transform.pos.x, 20.0);

        let _ = history.undo(&redone).unwrap();
        let mut branch = base.clone();
        branch.models[0].transform.pos.x = -8.0;
        history.mark_changed();
        history.flush(branch.clone());
        assert!(!history.can_redo());
        assert_eq!(
            history.undo(&branch).unwrap().models[0].transform.pos.x,
            0.0
        );
    }

    #[test]
    #[ignore = "uses the supplied high-resolution Body.stl and writes a large project"]
    fn supplied_body_project_roundtrip() {
        let source = std::path::Path::new(
            "/run/media/makzumi/storage/Projects/iCarControls-3/case-2/HighRes_STLs/Body.stl",
        );
        let body = mesh::load_model(source).unwrap();
        let triangles = body.tris.len();
        let mut project = sample();
        project.models = vec![ProjectModel {
            mesh: Arc::new(body),
            transform: Transform::default(),
        }];
        let path = std::env::temp_dir().join("resin_slicer_body_project.rslice");
        project.save(&path).unwrap();
        let loaded = Project::read(&path).unwrap();
        assert_eq!(loaded.models[0].mesh.tris.len(), triangles);
        assert!(!loaded.models[0].mesh.verts.is_empty());
        if std::env::var_os("KEEP_FORMAT_SAMPLES").is_none() {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn snapshots_share_mesh_storage() {
        let project = sample();
        let snapshot = project.clone();
        assert!(Arc::ptr_eq(
            &project.models[0].mesh,
            &snapshot.models[0].mesh
        ));
    }
}
