//! Where snapshots live: one JSON file each, named by when it was taken, in
//! `$KIT_STATE_DIR`, else `$XDG_STATE_HOME/kit`, else `~/.local/state/kit`.
//!
//! Snapshots record command lines and kitty's view of each window's
//! environment, which can hold secrets, so the directory is private to the
//! user.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::snapshot::Snapshot;

pub struct Store {
    dir: PathBuf,
}

/// A snapshot on disk.
pub struct Entry {
    pub path: PathBuf,
    pub snapshot: Snapshot,
}

impl Store {
    pub fn open_default() -> Result<Store> {
        Ok(Store::at(default_dir()?))
    }

    pub fn at(root: PathBuf) -> Store {
        Store { dir: root.join("snapshots") }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Writes a snapshot atomically and returns its path.
    pub fn save(&self, snap: &Snapshot) -> Result<PathBuf> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        let stem = snap.taken_at.format("%Y-%m-%dT%H-%M-%SZ").to_string();
        let mut path = self.dir.join(format!("{stem}.json"));
        let mut n = 1;
        while path.exists() {
            n += 1;
            path = self.dir.join(format!("{stem}-{n}.json"));
        }
        let tmp = path.with_extension("json.tmp");
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        serde_json::to_writer_pretty(&mut f, snap)?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        fs::rename(&tmp, &path)?;
        Ok(path)
    }

    /// Every readable snapshot, newest first. Unreadable files are reported
    /// and skipped.
    pub fn list(&self) -> Result<Vec<Entry>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", self.dir.display())),
        };
        let mut out = Vec::new();
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "json") {
                continue;
            }
            match load(&path) {
                Ok(snapshot) => out.push(Entry { path, snapshot }),
                Err(err) => eprintln!("kit: skipping {}: {err:#}", path.display()),
            }
        }
        out.sort_by(|a, b| b.snapshot.taken_at.cmp(&a.snapshot.taken_at).then(b.path.cmp(&a.path)));
        Ok(out)
    }

    /// Finds a snapshot by `latest`, by its number in `kit ls` (1 is the
    /// newest), or by path.
    pub fn resolve(&self, reference: &str) -> Result<Entry> {
        if reference == "latest" {
            return self.list()?.into_iter().next().ok_or_else(|| anyhow!("no snapshots yet"));
        }
        if let Ok(n) = reference.parse::<usize>() {
            let all = self.list()?;
            let count = all.len();
            return all
                .into_iter()
                .nth(n.checked_sub(1).ok_or_else(|| anyhow!("snapshots are numbered from 1"))?)
                .ok_or_else(|| anyhow!("there are only {count} snapshots"));
        }
        let path = PathBuf::from(reference);
        if !path.exists() {
            bail!("no snapshot {reference:?}: give a path, a number from `kit ls`, or `latest`");
        }
        Ok(Entry { snapshot: load(&path)?, path })
    }

    /// Deletes all but the newest `keep` snapshots, returning how many went.
    pub fn prune(&self, keep: usize) -> Result<usize> {
        let mut removed = 0;
        for e in self.list()?.into_iter().skip(keep) {
            fs::remove_file(&e.path).with_context(|| format!("removing {}", e.path.display()))?;
            removed += 1;
        }
        Ok(removed)
    }
}

pub fn load(path: &Path) -> Result<Snapshot> {
    let data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let snap: Snapshot = serde_json::from_slice(&data).with_context(|| format!("parsing {}", path.display()))?;
    if snap.format > crate::snapshot::FORMAT {
        bail!(
            "{} was written by a newer kit (format {}); upgrade kit to read it",
            path.display(),
            snap.format
        );
    }
    Ok(snap)
}

fn default_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("KIT_STATE_DIR") {
        return Ok(PathBuf::from(d));
    }
    if let Some(d) = std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d).join("kit"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".local/state/kit"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::tests::fixture;
    use chrono::Duration;

    #[test]
    fn saves_lists_resolves_and_prunes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().to_path_buf());
        assert!(store.list().unwrap().is_empty());
        assert!(store.resolve("latest").is_err());

        let mut s = fixture();
        let mut paths = Vec::new();
        for i in 0..3 {
            s.taken_at = fixture().taken_at + Duration::hours(i);
            paths.push(store.save(&s).unwrap());
        }
        // Two in the same second get distinct names.
        paths.push(store.save(&s).unwrap());
        assert_ne!(paths[2], paths[3]);

        let mode = fs::metadata(&paths[0]).unwrap().permissions();
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777, 0o600);

        let all = store.list().unwrap();
        assert_eq!(all.len(), 4);
        assert!(all[0].snapshot.taken_at >= all[3].snapshot.taken_at);

        assert_eq!(store.resolve("latest").unwrap().path, all[0].path);
        assert_eq!(store.resolve("4").unwrap().path, paths[0]);
        assert!(store.resolve("0").is_err());
        assert!(store.resolve("9").is_err());
        assert_eq!(store.resolve(paths[1].to_str().unwrap()).unwrap().path, paths[1]);

        assert_eq!(store.prune(2).unwrap(), 2);
        assert_eq!(store.list().unwrap().len(), 2);
        assert!(!paths[0].exists());
    }

    #[test]
    fn refuses_newer_formats() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = fixture();
        s.format = crate::snapshot::FORMAT + 1;
        let path = dir.path().join("future.json");
        fs::write(&path, serde_json::to_vec(&s).unwrap()).unwrap();
        assert!(load(&path).unwrap_err().to_string().contains("newer kit"));
    }
}
