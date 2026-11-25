//! Named bundles of the settings that follow the printer, not the model:
//! everything the Supports and Export tabs show, kept in the user's config
//! folder so any project can load them back.

use crate::config::{JobFormat, PrintSettings};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One saved configuration: the name it goes by, the whole Layer & exposure
/// card (which carries the Supports tab inside it), and the export tab's
/// format choices. The file name and folder belong to the project, so they
/// stay out.
#[derive(Clone, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub print: PrintSettings,
    /// `None` is the printer's native format.
    pub format: Option<JobFormat>,
    /// Write the layer masks alongside the job.
    pub also_masks: bool,
}

impl Preset {
    /// The values as a one-line summary for a hover, the way a menu shows
    /// what a row holds without opening it.
    pub fn summary(&self) -> String {
        format!(
            "{:.2} mm layers, {:.1} s exposure, {:.1} s initial ×{}",
            self.print.layer_height,
            self.print.exposure,
            self.print.bottom_exposure,
            self.print.bottom_layers
        )
    }
}

/// The named configurations on disk, kept as one readable JSON file next to
/// the recent lists.
#[derive(Default)]
pub struct Presets {
    presets: Vec<Preset>,
    file: PathBuf,
}

impl Presets {
    #[cfg(not(test))]
    pub fn load() -> Self {
        Self::read(&crate::recent::store_path("presets"))
    }

    /// A store at a chosen file, for tests.
    #[cfg(test)]
    pub fn at(file: impl Into<PathBuf>) -> Self {
        Self::read(&file.into())
    }

    fn read(file: &Path) -> Self {
        // A store that cannot be read — missing or mangled — simply starts
        // empty; the first save rewrites the file in good order.
        let presets = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            presets,
            file: file.to_path_buf(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
    }

    pub fn names(&self) -> Vec<String> {
        self.presets.iter().map(|p| p.name.clone()).collect()
    }

    pub fn find(&self, name: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.name == name)
    }

    /// Add a preset, or replace the one that already answers to the name.
    pub fn save(&mut self, preset: Preset) {
        match self.presets.iter_mut().find(|p| p.name == preset.name) {
            Some(slot) => *slot = preset,
            None => self.presets.push(preset),
        }
        self.write();
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.presets.len();
        self.presets.retain(|p| p.name != name);
        if self.presets.len() < before {
            self.write();
            return true;
        }
        false
    }

    fn write(&self) {
        let Ok(text) = serde_json::to_string_pretty(&self.presets) else {
            return;
        };
        if self
            .file
            .parent()
            .is_some_and(|dir| std::fs::create_dir_all(dir).is_err())
        {
            return;
        }
        let tmp = self.file.with_extension("json.tmp");
        if std::fs::write(&tmp, text)
            .and_then(|_| std::fs::rename(&tmp, &self.file))
            .is_err()
        {
            // Nothing is lost: the values stay in the window, unsaved.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PrintSettings;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    fn named(name: &str) -> Preset {
        Preset {
            name: name.into(),
            print: PrintSettings::default(),
            format: None,
            also_masks: false,
        }
    }

    #[test]
    fn presets_round_trip_through_the_file() {
        let file = temp("pinto_slicer_presets_round_trip.json");
        let _ = std::fs::remove_file(&file);
        let mut store = Presets::at(file.clone());
        assert!(store.is_empty(), "a missing store reads as empty");
        let mut fast = named("Fast print");
        fast.print.exposure = 2.5;
        store.save(fast);
        store.save(named("Slow and careful"));
        // The same name replaces rather than duplicates.
        let mut again = named("Fast print");
        again.print.exposure = 3.0;
        store.save(again);
        let reopened = Presets::at(&file);
        assert_eq!(reopened.names().len(), 2, "{:?}", reopened.names());
        assert!((reopened.find("Fast print").unwrap().print.exposure - 3.0).abs() < 1e-4);
        assert!(reopened.find("Slow and careful").is_some());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn remove_drops_only_the_named_one() {
        let file = temp("pinto_slicer_presets_remove.json");
        let _ = std::fs::remove_file(&file);
        let mut store = Presets::at(file.clone());
        store.save(named("one"));
        store.save(named("two"));
        assert!(store.remove("one"));
        assert!(!store.remove("one"));
        assert_eq!(store.names(), vec!["two".to_string()]);
        let reopened = Presets::at(&file);
        assert_eq!(reopened.names(), vec!["two".to_string()]);
        let _ = std::fs::remove_file(&file);
    }
}
