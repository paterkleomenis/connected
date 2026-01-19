use connected_core::Result;
use connected_core::filesystem::{FilesystemProvider, FsEntry, FsEntryType};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

pub struct DesktopFilesystemProvider {
    root: PathBuf,
    root_canonical: PathBuf,
}

impl DesktopFilesystemProvider {
    pub fn new() -> Self {
        let root = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let root_canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
        Self {
            root,
            root_canonical,
        }
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let safe_path = path.trim_start_matches('/');
        if Path::new(safe_path)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(connected_core::ConnectedError::Filesystem(
                "Invalid path".to_string(),
            ));
        }

        let full_path = if safe_path.is_empty() {
            self.root.clone()
        } else {
            self.root.join(safe_path)
        };

        let canonical = if full_path.exists() {
            full_path
                .canonicalize()
                .map_err(connected_core::ConnectedError::Io)?
        } else if let Some(parent) = full_path.parent() {
            let parent_canon = parent
                .canonicalize()
                .map_err(connected_core::ConnectedError::Io)?;
            match full_path.file_name() {
                Some(name) => parent_canon.join(name),
                None => parent_canon,
            }
        } else {
            full_path.clone()
        };

        if !canonical.starts_with(&self.root_canonical) {
            return Err(connected_core::ConnectedError::Filesystem(
                "Path escapes root".to_string(),
            ));
        }

        Ok(full_path)
    }
}

impl FilesystemProvider for DesktopFilesystemProvider {
    fn list_dir(&self, path: &str) -> Result<Vec<FsEntry>> {
        let full_path = self.resolve_path(path)?;

        let mut entries = Vec::new();
        let read_dir = fs::read_dir(full_path).map_err(connected_core::ConnectedError::Io)?;

        for entry_result in read_dir {
            let entry = entry_result.map_err(connected_core::ConnectedError::Io)?;
            let metadata = entry
                .metadata()
                .map_err(connected_core::ConnectedError::Io)?;
            let file_name = entry.file_name().to_string_lossy().to_string();

            let entry_type = if metadata.is_dir() {
                FsEntryType::Directory
            } else if metadata.is_symlink() {
                FsEntryType::Symlink
            } else {
                FsEntryType::File
            };

            let size = metadata.len();
            let modified = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());

            // Construct relative path for the entry
            // This is just the name if listing directory
            let entry_path = if path == "/" || path.is_empty() {
                format!("/{}", file_name)
            } else {
                format!("{}/{}", path.trim_end_matches('/'), file_name)
            };

            entries.push(FsEntry {
                name: file_name,
                path: entry_path,
                entry_type,
                size,
                modified,
            });
        }

        Ok(entries)
    }

    fn read_file(&self, path: &str, offset: u64, size: u64) -> Result<Vec<u8>> {
        let full_path = self.resolve_path(path)?;
        let mut file = fs::File::open(full_path).map_err(connected_core::ConnectedError::Io)?;

        use std::io::{Read, Seek, SeekFrom};
        file.seek(SeekFrom::Start(offset))
            .map_err(connected_core::ConnectedError::Io)?;

        let mut buffer = vec![0u8; size as usize];
        let read = file
            .read(&mut buffer)
            .map_err(connected_core::ConnectedError::Io)?;

        buffer.truncate(read);
        Ok(buffer)
    }

    fn write_file(&self, path: &str, offset: u64, data: &[u8]) -> Result<u64> {
        let full_path = self.resolve_path(path)?;

        use std::io::{Seek, SeekFrom, Write};

        // Create if not exists, open for write
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(full_path)
            .map_err(connected_core::ConnectedError::Io)?;

        file.seek(SeekFrom::Start(offset))
            .map_err(connected_core::ConnectedError::Io)?;
        file.write_all(data)
            .map_err(connected_core::ConnectedError::Io)?;

        Ok(data.len() as u64)
    }

    fn get_metadata(&self, path: &str) -> Result<FsEntry> {
        let full_path = self.resolve_path(path)?;
        let metadata = fs::metadata(&full_path).map_err(connected_core::ConnectedError::Io)?;

        let file_name = full_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let entry_type = if metadata.is_dir() {
            FsEntryType::Directory
        } else if metadata.is_symlink() {
            FsEntryType::Symlink
        } else {
            FsEntryType::File
        };

        let size = metadata.len();
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        Ok(FsEntry {
            name: file_name,
            path: path.to_string(),
            entry_type,
            size,
            modified,
        })
    }

    fn get_thumbnail(&self, path: &str) -> Result<Vec<u8>> {
        let full_path = self.resolve_path(path)?;

        // Basic extension check
        let ext = full_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        if !["jpg", "jpeg", "png", "gif", "webp", "bmp", "ico"].contains(&ext.as_str()) {
            return Ok(Vec::new());
        }

        // We use a closure to map image errors to our error type
        let generate = || -> std::result::Result<Vec<u8>, image::ImageError> {
            let img = image::open(&full_path)?;
            let thumb = img.thumbnail(96, 96);

            let mut buffer = std::io::Cursor::new(Vec::new());
            thumb.write_to(&mut buffer, image::ImageFormat::Jpeg)?;
            Ok(buffer.into_inner())
        };

        match generate() {
            Ok(data) => Ok(data),
            Err(e) => {
                // Log warning but don't fail hard, just return empty or error
                // returning error allows caller to handle it (e.g. show default icon)
                // but "NotImplemented" is what caused the issue before.
                // Let's return a specific error or empty if we just can't decode it.
                // A specialized error would be better but Filesystem(String) is fine.
                Err(connected_core::ConnectedError::Filesystem(format!(
                    "Thumbnail generation failed: {}",
                    e
                )))
            }
        }
    }
}
