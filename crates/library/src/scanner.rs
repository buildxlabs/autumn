//! Walks library folders and parses video names.
//!
//! Hidden folders and paths matched by a `.seaignore` file at the folder root
//! are skipped. Each file keeps its path, size, modification time and parsed
//! fields; a file with no title in its name borrows one from its parent folder.

use std::{
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use release_parser::{EpisodeKind, Release, parse};

use crate::{LibraryError, normalize::normalize};

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "webm", "m4v", "ts", "m2ts", "flv", "wmv",
];

/// A video found by a walk, with its parsed fields.
#[derive(Debug, Clone, PartialEq)]
pub struct ScannedFile {
    /// Where the file lives.
    pub path: PathBuf,
    /// Size in bytes.
    pub size: i64,
    /// Modification time in seconds since the Unix epoch.
    pub modified: i64,
    /// What the release parser read from the name and parent folders.
    pub release: Release,
    /// The normalized title used to group this file with its siblings.
    pub group_key: String,
}

/// Walks one library folder, skipping hidden paths and ignored patterns.
///
/// # Errors
///
/// [`LibraryError::Walk`] when a directory or file cannot be read.
pub fn walk(folder: &Path) -> Result<Vec<ScannedFile>, LibraryError> {
    let patterns = read_ignore(folder);
    let mut files = Vec::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<_> = fs::read_dir(&dir)
            .map_err(|source| LibraryError::Walk {
                path: dir.clone(),
                source,
            })?
            .filter_map(Result::ok)
            .collect();
        // Sort so a scan has a stable order for progress and tests.
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            let relative = path
                .strip_prefix(folder)
                .map_err(|_| LibraryError::Walk {
                    path: path.clone(),
                    source: std::io::Error::other("file outside its library folder"),
                })?
                .to_string_lossy()
                .into_owned();
            if is_ignored(&relative, &patterns) {
                continue;
            }
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                stack.push(path);
                continue;
            }
            if let Some(file) = scanned(&path)? {
                files.push(file);
            }
        }
    }
    Ok(files)
}

fn read_ignore(folder: &Path) -> Vec<String> {
    let path = folder.join(".seaignore");
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

fn scanned(path: &Path) -> Result<Option<ScannedFile>, LibraryError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_lowercase();
    if !VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        return Ok(None);
    }
    let metadata = fs::metadata(path).map_err(|source| LibraryError::Walk {
        path: path.to_path_buf(),
        source,
    })?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        });
    let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);

    let mut release = parse(name);
    if release.title.is_none() {
        // The nearest named parent folder is the title when the file name is
        // only a number, as in `Series Name/Season 2/- 01.mkv`.
        for parent in path.ancestors().skip(1) {
            let Some(folder) = parent.file_name().and_then(|name| name.to_str()) else {
                break;
            };
            let parsed = parse(folder);
            if let Some(title) = parsed.title {
                release.title = Some(title);
                release.season = release.season.or(parsed.season);
                break;
            }
        }
    }
    let group_key = release
        .title
        .as_deref()
        .map(normalize)
        .map_or_else(String::new, |title| title.key);
    Ok(Some(ScannedFile {
        path: path.to_path_buf(),
        size,
        modified,
        release,
        group_key,
    }))
}

fn is_ignored(relative: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| glob(pattern, relative))
}

/// Small `*` and `?` glob, enough for `.seaignore` lines.
fn glob(pattern: &str, text: &str) -> bool {
    match pattern.chars().next() {
        None => text.is_empty(),
        Some('*') => {
            let rest = pattern.strip_prefix('*').unwrap_or_default();
            (0..=text.chars().count()).any(|skip| {
                let tail: String = text.chars().skip(skip).collect();
                glob(rest, &tail)
            })
        }
        Some('?') => {
            let mut chars = text.chars();
            chars.next().is_some()
                && glob(
                    pattern.strip_prefix('?').unwrap_or_default(),
                    chars.as_str(),
                )
        }
        Some(c) => match (pattern.strip_prefix(c), text.strip_prefix(c)) {
            (Some(rest), Some(rest_text)) => glob(rest, rest_text),
            _ => false,
        },
    }
}

/// The storage form of a parsed episode kind.
pub(crate) fn kind_name(kind: EpisodeKind) -> &'static str {
    match kind {
        EpisodeKind::Episode => "episode",
        EpisodeKind::Special => "special",
        EpisodeKind::Movie => "movie",
        EpisodeKind::Ncop => "ncop",
        EpisodeKind::Nced => "nced",
        _ => "extra",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{glob, walk};

    #[test]
    fn glob_matches_wildcards() {
        assert!(glob("*.mkv", "show/- 01.mkv"));
        assert!(glob("extras/*", "extras/- 01.mkv"));
        assert!(!glob("extras/*", "show/- 01.mkv"));
        assert!(glob("show/?02.mkv", "show/102.mkv"));
    }

    #[test]
    fn skips_hidden_folders_and_ignored_paths() {
        let dir = tempdir().unwrap();
        let show = dir.path().join("show");
        fs::create_dir_all(show.join(".hidden")).unwrap();
        fs::create_dir_all(show.join("extras")).unwrap();
        fs::write(show.join("- 01.mkv"), b"").unwrap();
        fs::write(show.join(".hidden/- 02.mkv"), b"").unwrap();
        fs::write(show.join("extras/- 03.mkv"), b"").unwrap();
        fs::write(show.join("notes.txt"), b"").unwrap();
        fs::write(show.join(".seaignore"), "extras/*\n").unwrap();

        let files = walk(&show).unwrap();
        assert_eq!(files.len(), 1, "only the video outside ignored folders");
        assert!(
            files
                .first()
                .is_some_and(|file| file.path.ends_with("- 01.mkv"))
        );
    }

    #[test]
    fn borrows_the_folder_title_when_the_name_has_none() {
        let dir = tempdir().unwrap();
        let show = dir.path().join("[Group] Series Alpha");
        fs::create_dir_all(&show).unwrap();
        fs::write(show.join("- 01.mkv"), b"").unwrap();

        let files = walk(&show).unwrap();
        let file = files.first().unwrap();
        assert_eq!(file.release.title.as_deref(), Some("Series Alpha"));
        assert_eq!(file.group_key, "series alpha");
    }
}
