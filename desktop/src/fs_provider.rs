use connected_core::Result;
use connected_core::filesystem::{FilesystemProvider, FsEntry, FsEntryType};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

pub struct DesktopFilesystemProvider {
    root: PathBuf,
    root_canonical: PathBuf,
}

const MAX_FS_READ_BYTES: u64 = 4 * 1024 * 1024;

impl DesktopFilesystemProvider {
    pub fn new() -> Self {
        let root = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let root_canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
        Self {
            root,
            root_canonical,
        }
    }

    /// Resolve and validate a user-supplied path, returning the **canonical** path.
    ///
    /// Returning the canonical path (rather than the un-canonicalized `full_path`)
    /// eliminates a TOCTOU race: previously the canonical path was used only for
    /// the containment check, while operations used `full_path`. An attacker could
    /// insert a symlink between the check and the operation to escape the root.
    /// Now the same canonical path is used for both validation and all subsequent
    /// filesystem operations.
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

        // Return the canonical path so that all subsequent operations use the
        // validated path, closing the TOCTOU window.
        Ok(canonical)
    }
}

impl FilesystemProvider for DesktopFilesystemProvider {
    fn list_dir(&self, path: &str) -> Result<Vec<FsEntry>> {
        let full_path = self.resolve_path(path)?;

        let mut entries = Vec::new();
        let read_dir = fs::read_dir(full_path).map_err(connected_core::ConnectedError::Io)?;

        for entry_result in read_dir {
            let entry = entry_result.map_err(connected_core::ConnectedError::Io)?;
            let file_type = entry
                .file_type()
                .map_err(connected_core::ConnectedError::Io)?;
            let metadata = entry
                .metadata()
                .map_err(connected_core::ConnectedError::Io)?;
            let file_name = entry.file_name().to_string_lossy().to_string();

            // Use file_type() (which does NOT follow symlinks) for type detection,
            // rather than metadata() which follows symlinks and would misreport
            // symlinks as regular files or directories.
            let entry_type = if file_type.is_symlink() {
                FsEntryType::Symlink
            } else if file_type.is_dir() {
                FsEntryType::Directory
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
        if size > MAX_FS_READ_BYTES {
            return Err(connected_core::ConnectedError::Filesystem(format!(
                "Read size {} exceeds limit {}",
                size, MAX_FS_READ_BYTES
            )));
        }
        let full_path = self.resolve_path(path)?;

        // Verify the resolved path is a regular file (not a device node, socket,
        // FIFO, etc.) to avoid reading from special files.
        let meta = fs::symlink_metadata(&full_path).map_err(connected_core::ConnectedError::Io)?;
        if !meta.is_file() {
            return Err(connected_core::ConnectedError::Filesystem(format!(
                "Not a regular file: {}",
                full_path.display()
            )));
        }

        let mut file = fs::File::open(full_path).map_err(connected_core::ConnectedError::Io)?;

        use std::io::{Read, Seek, SeekFrom};
        file.seek(SeekFrom::Start(offset))
            .map_err(connected_core::ConnectedError::Io)?;

        let size_usize = usize::try_from(size).map_err(|_| {
            connected_core::ConnectedError::Filesystem("Read size is too large".to_string())
        })?;
        let mut buffer = vec![0u8; size_usize];
        let read = file
            .read(&mut buffer)
            .map_err(connected_core::ConnectedError::Io)?;

        buffer.truncate(read);
        Ok(buffer)
    }

    fn write_file(&self, _path: &str, _offset: u64, _data: &[u8]) -> Result<u64> {
        // Defense-in-depth: Remote writes under $HOME are too dangerous.
        // A trusted peer could overwrite ~/.ssh/authorized_keys, ~/.bashrc, crontabs, etc.
        // The protocol dispatcher also rejects WriteFileRequest, but we enforce the policy
        // here at the provider level too, so that even if the dispatcher guard is accidentally
        // removed or bypassed, writes are still blocked.
        Err(connected_core::ConnectedError::Filesystem(
            "Remote file writes are disabled for security reasons".to_string(),
        ))
    }

    fn get_metadata(&self, path: &str) -> Result<FsEntry> {
        let full_path = self.resolve_path(path)?;
        // Use symlink_metadata() instead of metadata() so that symlinks are correctly
        // detected. fs::metadata() follows symlinks and would misreport them as regular
        // files or directories, potentially allowing path escape / information leakage.
        let metadata =
            fs::symlink_metadata(&full_path).map_err(connected_core::ConnectedError::Io)?;

        let file_name = full_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Check symlink first since symlink_metadata does not follow symlinks,
        // so is_symlink() will return true for symlinks.
        let entry_type = if metadata.is_symlink() {
            FsEntryType::Symlink
        } else if metadata.is_dir() {
            FsEntryType::Directory
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

        // Maximum image dimensions we're willing to decode for thumbnail generation.
        // Decoding extremely large images (e.g. 100000x100000) can exhaust memory.
        const MAX_THUMBNAIL_DIMENSION: u32 = 16384;

        // We use a closure to map image errors to our error type
        let generate = || -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
            // Check image dimensions before fully decoding to prevent memory exhaustion
            // from extremely large images.
            let reader = image::ImageReader::open(&full_path)?;
            let (width, height) = reader.into_dimensions()?;
            if width > MAX_THUMBNAIL_DIMENSION || height > MAX_THUMBNAIL_DIMENSION {
                return Err(format!(
                    "Image too large for thumbnail: {}x{} (max {}x{})",
                    width, height, MAX_THUMBNAIL_DIMENSION, MAX_THUMBNAIL_DIMENSION
                )
                .into());
            }

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
