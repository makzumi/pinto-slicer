//! The desktop's own file chooser.
//!
//! A hand-drawn browser inside an egui window looks foreign and has to
//! re-invent places, filters and sorting, so the chooser is delegated to the
//! platform — on Wayland that means the XDG portal, which hands the request to
//! whichever desktop is running (KDE's file dialog here).
//!
//! Dialogs run on their own thread so painting continues. `zbus`, underneath
//! the portal, needs one Tokio reactor that lives as long as the process.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

const MODELS: &str = "3D models (*.stl, *.obj)";
pub const MODEL_EXTENSIONS: [&str; 2] = ["stl", "obj"];
const PROJECTS: &str = "Pinto Slicer projects (*.rslice)";
pub const PROJECT_EXTENSION: &str = "rslice";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickKind {
    Models,
    OpenProject,
    SaveProject,
    ExportJob {
        extension: &'static str,
        label: &'static str,
    },
}

pub struct Answer {
    pub kind: PickKind,
    pub paths: Vec<PathBuf>,
}

struct Pending {
    kind: PickKind,
    rx: Receiver<Vec<PathBuf>>,
}

/// The file chooser and whatever it has already answered.
#[derive(Default)]
pub struct Picker {
    pending: Option<Pending>,
    /// The folder the last answer came from, so the next dialog opens there.
    last_dir: Option<PathBuf>,
}

impl Picker {
    pub fn new() -> Self {
        Self::default()
    }

    /// True while a dialog is up, so a second click does nothing.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    fn request(&mut self, kind: PickKind, start: Option<PathBuf>, suggested: Option<String>) {
        if self.busy() {
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let picked = ask(kind, start, suggested).unwrap_or_default();
            let _ = tx.send(picked);
        });
        self.pending = Some(Pending { kind, rx });
    }

    pub fn open_models(&mut self) {
        let start = self.last_dir.clone();
        self.request(PickKind::Models, start, None);
    }

    pub fn open_project(&mut self) {
        let start = self.last_dir.clone();
        self.request(PickKind::OpenProject, start, None);
    }

    pub fn save_project(&mut self, current: Option<PathBuf>) {
        let start = current.or_else(|| self.last_dir.clone());
        self.request(PickKind::SaveProject, start, None);
    }

    /// Ask for the single destination file of a printer job. The desktop's own
    /// save dialog supplies both the name and the folder.
    pub fn export_job(
        &mut self,
        stem: &str,
        extension: &'static str,
        label: &'static str,
        start: Option<PathBuf>,
    ) {
        self.request(
            PickKind::ExportJob { extension, label },
            start,
            Some(format!("{stem}.{extension}")),
        );
    }

    /// Whatever the user picked, once. Empty until the dialog is answered.
    pub fn poll(&mut self) -> Option<Answer> {
        let pending = self.pending.as_ref()?;
        match pending.rx.try_recv() {
            Ok(paths) => {
                if let Some(dir) = paths.first().and_then(|p| p.parent()) {
                    self.last_dir = Some(dir.to_path_buf());
                }
                let kind = pending.kind;
                self.pending = None;
                Some(Answer { kind, paths })
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                None
            }
        }
    }
}

/// The reactor the portal runs on.
///
/// The connection the portal is reached over is cached for the life of the
/// process, and the task that watches it belongs to the runtime that opened it.
/// A runtime built for one dialog and dropped afterwards therefore leaves the
/// cache pointing at a dead reactor: the first chooser answers, every later one
/// hangs unanswered. One runtime, made once, for the whole run.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a Tokio runtime for the desktop file chooser")
    })
}

/// Put the question to the desktop and wait for an answer.
fn ask(kind: PickKind, start: Option<PathBuf>, suggested: Option<String>) -> Option<Vec<PathBuf>> {
    let runtime = runtime();
    let _guard = runtime.enter();
    runtime.block_on(async move {
        let mut dialog = rfd::AsyncFileDialog::new().set_title(match kind {
            PickKind::Models => "Open models",
            PickKind::OpenProject => "Open project",
            PickKind::SaveProject => "Save project",
            PickKind::ExportJob { .. } => "Export job file",
        });
        dialog = match kind {
            PickKind::Models => dialog.add_filter(MODELS, &MODEL_EXTENSIONS),
            PickKind::OpenProject | PickKind::SaveProject => {
                dialog.add_filter(PROJECTS, &[PROJECT_EXTENSION])
            }
            PickKind::ExportJob { extension, label } => {
                dialog.add_filter(format!("{label} (*.{extension})"), &[extension])
            }
        };
        if let Some(start) = start {
            if start.is_dir() {
                dialog = dialog.set_directory(start);
            } else if let Some(dir) = start.parent() {
                dialog = dialog.set_directory(dir);
            }
        }
        match kind {
            PickKind::SaveProject => {
                dialog = dialog.set_file_name(suggested.as_deref().unwrap_or("project.rslice"));
            }
            PickKind::ExportJob { .. } => {
                if let Some(name) = &suggested {
                    dialog = dialog.set_file_name(name);
                }
            }
            _ => {}
        }
        let picked = match kind {
            PickKind::Models => dialog.pick_files().await,
            PickKind::OpenProject => dialog.pick_file().await.map(|file| vec![file]),
            PickKind::SaveProject | PickKind::ExportJob { .. } => {
                dialog.save_file().await.map(|file| vec![file])
            }
        }?;
        Some(picked.iter().map(|f| f.path().to_path_buf()).collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One dialog per run is fine, many dialogs are not: the portal's
    /// connection stays cached, so a second runtime would leave the second
    /// chooser talking to a dead reactor and it would never answer.
    #[test]
    fn the_chooser_keeps_the_same_reactor_for_every_dialog() {
        let first = runtime() as *const tokio::runtime::Runtime;
        let second = runtime() as *const tokio::runtime::Runtime;
        assert_eq!(
            first, second,
            "a new runtime per dialog breaks the second one"
        );
    }
}
