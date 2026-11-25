//! All file access of Phanes: reading, writing, listing, creating, renaming,
//! duplicating, trashing and revealing files, and watching folders for
//! changes made outside the app. The frontend reaches these only through
//! `src/lib/fs.ts`.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use notify::event::ModifyKind;
use notify::{Event, EventKind, EventKindMask, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

type Result<T> = std::result::Result<T, String>;

/// Names never shown in the workspace tree.
const IGNORED: &[&str] = &[".git", "node_modules"];
const MARKDOWN: &[&str] = &["md", "markdown"];
const IMAGES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "bmp", "ico"];

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Directory,
    Markdown,
    Image,
    Other,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    name: String,
    path: String,
    kind: EntryKind,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathInfo {
    exists: bool,
    is_dir: bool,
}

fn err(context: &str, path: &Path, e: io::Error) -> String {
    let what = match e.kind() {
        ErrorKind::NotFound => "not found".to_string(),
        ErrorKind::PermissionDenied => "permission denied".to_string(),
        ErrorKind::AlreadyExists => "already exists".to_string(),
        _ => e.to_string(),
    };
    format!("{context} {}: {what}", path.display())
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn kind_of(path: &Path, is_dir: bool) -> EntryKind {
    if is_dir {
        return EntryKind::Directory;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if MARKDOWN.contains(&ext.as_str()) {
        EntryKind::Markdown
    } else if IMAGES.contains(&ext.as_str()) {
        EntryKind::Image
    } else {
        EntryKind::Other
    }
}

/// List a folder for the tree: no dotfiles, `.git` or `node_modules`;
/// folders first, then files, each sorted case-insensitively.
#[tauri::command]
pub fn list_dir(path: String) -> Result<Vec<Entry>> {
    let dir = PathBuf::from(&path);
    let read = fs::read_dir(&dir).map_err(|e| err("Cannot list", &dir, e))?;
    let mut entries: Vec<Entry> = read
        .filter_map(|r| r.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            if name.starts_with('.') || IGNORED.contains(&name.as_str()) {
                return None;
            }
            let path = entry.path();
            // Follow symlinks so a linked folder shows as a folder.
            let is_dir = fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
            Some(Entry {
                kind: kind_of(&path, is_dir),
                path: path_string(&path),
                name,
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        let a_dir = a.kind == EntryKind::Directory;
        let b_dir = b.kind == EntryKind::Directory;
        b_dir
            .cmp(&a_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

#[tauri::command]
pub fn path_info(path: String) -> PathInfo {
    match fs::metadata(&path) {
        Ok(m) => PathInfo { exists: true, is_dir: m.is_dir() },
        Err(_) => PathInfo { exists: false, is_dir: false },
    }
}

/// Read a UTF-8 text file. A leading byte-order mark is dropped.
#[tauri::command]
pub fn read_text_file(path: String) -> Result<String> {
    let p = Path::new(&path);
    let bytes = fs::read(p).map_err(|e| err("Cannot open", p, e))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("Cannot open {}: it is not UTF-8 text", p.display()))?;
    Ok(text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text))
}

/// The user's Vim settings, `~/.config/phanes/vimrc` (docs/VIM.md), or None
/// when there is none.
#[tauri::command]
pub fn read_vimrc<R: Runtime>(app: AppHandle<R>) -> Result<Option<String>> {
    let dir = app.path().config_dir().map_err(|e| e.to_string())?;
    let p = dir.join("phanes").join("vimrc");
    match fs::read_to_string(&p) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(err("Cannot read", &p, e)),
    }
}

/// Write a text file, creating it if needed. Writes in place so permissions,
/// symlinks and hard links of the existing file are kept.
#[tauri::command]
pub fn write_text_file(path: String, contents: String) -> Result<()> {
    let p = Path::new(&path);
    fs::write(p, contents).map_err(|e| err("Cannot save", p, e))
}

/// Create a new empty file. Fails if something already exists at that path.
#[tauri::command]
pub fn create_file(path: String) -> Result<()> {
    let p = Path::new(&path);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(p)
        .map(|_| ())
        .map_err(|e| err("Cannot create", p, e))
}

#[tauri::command]
pub fn create_dir(path: String) -> Result<()> {
    let p = Path::new(&path);
    fs::create_dir(p).map_err(|e| err("Cannot create", p, e))
}

/// Rename or move a file or folder. Never overwrites an existing item.
#[tauri::command]
pub fn rename_path(from: String, to: String) -> Result<()> {
    let (from, to) = (Path::new(&from), Path::new(&to));
    if from == to {
        return Ok(());
    }
    if let Ok(existing) = to.symlink_metadata() {
        // Only allowed when `to` is the same item (a case-only rename on a
        // case-insensitive file system); never replace another file.
        if !same_file(from, &existing) {
            return Err(format!("Cannot rename: {} already exists", to.display()));
        }
    }
    fs::rename(from, to).map_err(|e| err("Cannot rename", from, e))
}

#[cfg(unix)]
fn same_file(path: &Path, other: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    path.symlink_metadata()
        .map(|m| m.dev() == other.dev() && m.ino() == other.ino())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn same_file(_path: &Path, _other: &fs::Metadata) -> bool {
    false
}

/// The first free "name copy.ext", "name copy 2.ext"… next to `path`.
fn duplicate_target(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("/"));
    let is_dir = path.is_dir();
    let (stem, ext) = if is_dir {
        (path.file_name().unwrap_or_default().to_string_lossy().into_owned(), None)
    } else {
        (
            path.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
            path.extension().map(|e| e.to_string_lossy().into_owned()),
        )
    };
    (1..)
        .map(|n| {
            let base = if n == 1 { format!("{stem} copy") } else { format!("{stem} copy {n}") };
            let name = match &ext {
                Some(ext) => format!("{base}.{ext}"),
                None => base,
            };
            parent.join(name)
        })
        .find(|candidate| candidate.symlink_metadata().is_err())
        .expect("an unused name always exists")
}

fn copy_recursive(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

/// Copy a file or folder next to itself and return the new path.
#[tauri::command]
pub fn duplicate_path(path: String) -> Result<String> {
    let from = Path::new(&path);
    let to = duplicate_target(from);
    copy_recursive(from, &to).map_err(|e| err("Cannot duplicate", from, e))?;
    Ok(path_string(&to))
}

/// Move to the system trash (GNOME Files' Trash). Never a permanent delete.
#[tauri::command]
pub fn trash_path(path: String) -> Result<()> {
    trash::delete(&path).map_err(|e| format!("Cannot move {path} to the trash: {e}"))
}

/// Show the item selected in the file manager.
#[tauri::command]
pub fn reveal_path(path: String) -> Result<()> {
    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(|e| format!("Cannot show {path}: {e}"))
}

/// Make a folder the workspace: check it, return its canonical path, and let
/// the preview load its images through the asset protocol.
#[tauri::command]
pub fn open_workspace<R: Runtime>(app: AppHandle<R>, path: String) -> Result<String> {
    let p = Path::new(&path);
    let canonical = fs::canonicalize(p).map_err(|e| err("Cannot open", p, e))?;
    if !canonical.is_dir() {
        return Err(format!("{} is not a folder", canonical.display()));
    }
    allow_assets(&app, &canonical)?;
    Ok(path_string(&canonical))
}

/// Let the preview load images from a folder (e.g. a file opened outside the workspace).
#[tauri::command]
pub fn allow_asset_dir<R: Runtime>(app: AppHandle<R>, path: String) -> Result<()> {
    allow_assets(&app, Path::new(&path))
}

fn allow_assets<R: Runtime>(app: &AppHandle<R>, dir: &Path) -> Result<()> {
    app.asset_protocol_scope()
        .allow_directory(dir, true)
        .map_err(|e| e.to_string())
}

/* ---------- Watching for outside changes ---------- */

/// Emitted with the paths that changed in the watched folders, one event per
/// burst of changes (`src/state/diskChanges.ts` takes it from there).
const CHANGED_EVENT: &str = "fs-changed";
/// A burst ends after this long without a new change…
const QUIET: Duration = Duration::from_millis(150);
/// …or after this long in all, so a long one (a `git checkout`) still shows up.
const MAX_BURST: Duration = Duration::from_secs(1);

/// The folders being watched, and the watcher, started on first use.
#[derive(Default)]
pub struct Watching(Mutex<Option<Watched>>);

struct Watched {
    watcher: RecommendedWatcher,
    dirs: HashSet<PathBuf>,
}

/// Watch exactly these folders, each without its subfolders: the workspace
/// root, its expanded folders and the folders of open files. Watching only
/// what Phanes shows keeps big trees (`node_modules`) out of it. A folder
/// that can't be watched (gone) is skipped and tried again next time.
#[tauri::command]
pub fn watch_dirs<R: Runtime>(app: AppHandle<R>, watching: State<'_, Watching>, dirs: Vec<String>) -> Result<()> {
    let mut guard = watching.0.lock().map_err(|_| "The file watcher stopped".to_string())?;
    if guard.is_none() {
        let watcher = start_watcher(app).map_err(|e| format!("Cannot watch for changes: {e}"))?;
        *guard = Some(Watched { watcher, dirs: HashSet::new() });
    }
    let Some(watched) = guard.as_mut() else { return Ok(()) };
    let wanted: HashSet<PathBuf> = dirs.into_iter().map(PathBuf::from).collect();
    let gone: Vec<PathBuf> = watched.dirs.difference(&wanted).cloned().collect();
    for dir in gone {
        let _ = watched.watcher.unwatch(&dir);
        watched.dirs.remove(&dir);
    }
    for dir in wanted {
        if !watched.dirs.contains(&dir) && watched.watcher.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
            watched.dirs.insert(dir);
        }
    }
    Ok(())
}

fn start_watcher<R: Runtime>(app: AppHandle<R>) -> notify::Result<RecommendedWatcher> {
    let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
    let watcher = new_watcher(move |paths| {
        let _ = tx.send(paths);
    })?;
    // Ends when the watcher (and with it the sender) is dropped.
    std::thread::spawn(move || {
        batch_changes(&rx, QUIET, MAX_BURST, |paths| {
            let _ = app.emit(CHANGED_EVENT, paths);
        })
    });
    Ok(watcher)
}

/// A watcher that hands over the paths of each content change.
fn new_watcher(on_change: impl Fn(Vec<PathBuf>) + Send + 'static) -> notify::Result<RecommendedWatcher> {
    // Content and names only: no open/close events, which reading a file
    // after a change would cause again, and no metadata (permissions, times).
    let kinds = EventKindMask::CREATE | EventKindMask::REMOVE | EventKindMask::MODIFY_DATA | EventKindMask::MODIFY_NAME;
    RecommendedWatcher::new(
        move |event: notify::Result<Event>| {
            if let Ok(event) = event {
                if is_content_change(&event.kind) {
                    on_change(event.paths);
                }
            }
        },
        notify::Config::default().with_event_kinds(kinds),
    )
}

/// Whether an event may change what a folder lists or what a file holds.
fn is_content_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Any | EventKind::Other => true,
        EventKind::Modify(modify) => !matches!(modify, ModifyKind::Metadata(_)),
        EventKind::Access(_) => false,
    }
}

/// Gather changes into bursts and hand each burst's paths, without
/// duplicates, to `emit`. Returns when the sending side is gone.
fn batch_changes(rx: &Receiver<Vec<PathBuf>>, quiet: Duration, max: Duration, mut emit: impl FnMut(Vec<String>)) {
    while let Ok(first) = rx.recv() {
        let started = Instant::now();
        let mut paths: BTreeSet<String> = first.iter().map(|p| path_string(p)).collect();
        let mut open = true;
        while started.elapsed() < max {
            match rx.recv_timeout(quiet) {
                Ok(more) => paths.extend(more.iter().map(|p| path_string(p))),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    open = false;
                    break;
                }
            }
        }
        emit(paths.into_iter().collect());
        if !open {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, DataChange, MetadataKind, RenameMode};

    #[test]
    fn content_changes_count_and_access_or_metadata_do_not() {
        assert!(is_content_change(&EventKind::Create(CreateKind::File)));
        assert!(is_content_change(&EventKind::Modify(ModifyKind::Data(DataChange::Any))));
        assert!(is_content_change(&EventKind::Modify(ModifyKind::Name(RenameMode::To))));
        assert!(!is_content_change(&EventKind::Access(AccessKind::Any)));
        assert!(!is_content_change(&EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any))));
    }

    /// What a real watcher on a temporary folder reports within `wait` after `act`.
    fn changes_after(dir: &Path, wait: Duration, act: impl FnOnce()) -> BTreeSet<PathBuf> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = new_watcher(move |paths| {
            let _ = tx.send(paths);
        })
        .unwrap();
        watcher.watch(dir, RecursiveMode::NonRecursive).unwrap();
        act();
        let mut seen = BTreeSet::new();
        let until = Instant::now() + wait;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(paths) => seen.extend(paths),
                Err(_) => break,
            }
        }
        seen
    }

    #[test]
    fn the_real_watcher_sees_saves_but_not_reads_or_subfolders() {
        let dir = std::env::temp_dir().join(format!("phanes-watch-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        let file = dir.join("notes.md");
        fs::write(&file, "one").unwrap();
        let wait = Duration::from_millis(400);

        // A plain write, like Phanes' own save.
        assert!(changes_after(&dir, wait, || fs::write(&file, "two").unwrap()).contains(&file));
        // An editor's save: write a temporary file, then rename it over the file.
        let saved = changes_after(&dir, wait, || {
            fs::write(dir.join(".notes.md.tmp"), "three").unwrap();
            fs::rename(dir.join(".notes.md.tmp"), &file).unwrap();
        });
        assert!(saved.contains(&file), "{saved:?}");
        // Reading after a change must not report a change again (no loop).
        assert!(changes_after(&dir, wait, || drop(fs::read_to_string(&file).unwrap())).is_empty());
        // Subfolders aren't watched: the tree watches each listed folder itself.
        assert!(changes_after(&dir, wait, || fs::write(dir.join("sub/other.md"), "x").unwrap()).is_empty());
        // Removing the file.
        assert!(changes_after(&dir, wait, || fs::remove_file(&file).unwrap()).contains(&file));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_burst_of_changes_is_one_batch_without_duplicates() {
        let (tx, rx) = mpsc::channel();
        let sender = std::thread::spawn(move || {
            tx.send(vec![PathBuf::from("/w/a.md")]).unwrap();
            tx.send(vec![PathBuf::from("/w/a.md"), PathBuf::from("/w/b.md")]).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            tx.send(vec![PathBuf::from("/w/c.md")]).unwrap();
        });
        let mut batches = Vec::new();
        batch_changes(&rx, Duration::from_millis(100), Duration::from_secs(5), |paths| batches.push(paths));
        sender.join().unwrap();
        assert_eq!(batches, vec![vec!["/w/a.md".to_string(), "/w/b.md".to_string()], vec!["/w/c.md".to_string()]]);
    }
}
