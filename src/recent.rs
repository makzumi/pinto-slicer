//! The files opened most recently, so the File menu can offer them again.
//!
//! The list lives in the desktop's own configuration folder —
//! `$XDG_CONFIG_HOME/pinto-slicer/recent` (usually `~/.config/pinto-slicer/recent`).
//! It is read once when the window opens and written whenever it changes. A
//! missing or damaged list only means an empty menu: nothing about it is worth
//! an error on the plate.

use std::path::{Path, PathBuf};

/// How many files the menu offers.
pub const LIMIT: usize = 10;

/// The recent-file list and the file it is kept in.
pub struct Recent {
    paths: Vec<PathBuf>,
    file: PathBuf,
}

impl Recent {
    /// Read the list from its usual place, dropping files that are gone.
    /// `name` picks the store: one list for models, one for projects.
    #[cfg(not(test))]
    pub fn load(name: &str) -> Self {
        let file = store_path(name);
        let mut recent = Self::read(&file);
        recent.prune();
        recent
    }

    fn read(file: &Path) -> Self {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let paths = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .take(LIMIT)
            .collect();
        Self {
            paths,
            file: file.to_path_buf(),
        }
    }

    /// A list kept in `file`, so a test never reads or writes the desktop's own.
    #[cfg(test)]
    pub fn at(file: PathBuf) -> Self {
        Self {
            paths: Vec::new(),
            file,
        }
    }

    /// The files, newest first.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Put `path` at the front, once, and keep the list short.
    pub fn remember(&mut self, path: &Path) {
        let path = same_file_key(path);
        self.paths.retain(|known| known != &path);
        self.paths.insert(0, path);
        self.paths.truncate(LIMIT);
        self.write();
    }

    /// Drop a file the person has moved or deleted since it was opened.
    pub fn forget(&mut self, path: &Path) {
        let path = same_file_key(path);
        let before = self.paths.len();
        self.paths.retain(|known| known != &path);
        if self.paths.len() != before {
            self.write();
        }
    }

    /// Drop rows whose file is gone. Only the menu changes here: opening the
    /// window must not have to write anything.
    fn prune(&mut self) {
        self.paths.retain(|path| path.exists());
    }

    fn write(&self) {
        let Some(dir) = self.file.parent() else {
            return;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let mut text = String::new();
        for path in &self.paths {
            text.push_str(&path.display().to_string());
            text.push('\n');
        }
        // Write beside the list and move it into place, so an interrupted write
        // cannot leave half a path behind.
        let temp = self.file.with_extension("tmp");
        if std::fs::write(&temp, text).is_ok() {
            let _ = std::fs::rename(&temp, &self.file);
        }
    }
}

/// One spelling per file, so the same model reached by two folders is one row.
fn same_file_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `$XDG_CONFIG_HOME/pinto-slicer/<name>`, else under the home folder.
#[cfg(not(test))]
fn store_path(name: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(config_home)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("pinto-slicer").join(name)
}

#[cfg(not(test))]
fn config_home() -> Option<PathBuf> {
    // Windows keeps application data in APPDATA rather than a dot folder.
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(appdata));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".config"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway store, in a folder of its own so parallel tests cannot meet.
    fn store(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pinto-recent-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp folder");
        dir.join("recent")
    }

    fn recent_at(file: &Path) -> Recent {
        Recent {
            paths: Vec::new(),
            file: file.to_path_buf(),
        }
    }

    #[test]
    fn remembered_files_come_first_once_and_only_ten() {
        let file = store("order");
        let mut recent = recent_at(&file);
        for i in 0..12 {
            recent.remember(Path::new(&format!("/models/part{i}.stl")));
        }
        assert_eq!(recent.paths().len(), LIMIT);
        assert_eq!(recent.paths()[0], PathBuf::from("/models/part11.stl"));
        assert_eq!(
            recent.paths()[LIMIT - 1],
            PathBuf::from("/models/part2.stl")
        );

        // Opening a file again moves it up instead of adding a second row.
        recent.remember(Path::new("/models/part5.stl"));
        assert_eq!(recent.paths().len(), LIMIT);
        assert_eq!(recent.paths()[0], PathBuf::from("/models/part5.stl"));
        assert_eq!(
            recent
                .paths()
                .iter()
                .filter(|path| path.as_path() == Path::new("/models/part5.stl"))
                .count(),
            1
        );

        // What was written is what the next run reads.
        let again = Recent::read(&file);
        assert_eq!(again.paths(), recent.paths());
        let _ = std::fs::remove_dir_all(file.parent().expect("a folder"));
    }

    #[test]
    fn a_missing_store_is_an_empty_list() {
        let file = store("missing").with_extension("not-there");
        assert!(Recent::read(&file).paths().is_empty());
        let _ = std::fs::remove_dir_all(file.parent().expect("a folder"));
    }

    #[test]
    fn forget_and_prune_drop_what_is_gone() {
        let file = store("gone");
        let keep = file.parent().expect("a folder").join("keep.stl");
        std::fs::write(&keep, b"solid").expect("a file");
        let mut recent = recent_at(&file);
        recent.remember(&keep);
        recent.remember(Path::new("/models/moved-away.stl"));

        recent.prune();
        assert_eq!(recent.paths(), [same_file_key(&keep)]);

        recent.remember(Path::new("/models/vanished.stl"));
        recent.forget(Path::new("/models/vanished.stl"));
        assert_eq!(recent.paths(), [same_file_key(&keep)]);
        let _ = std::fs::remove_dir_all(file.parent().expect("a folder"));
    }
}
