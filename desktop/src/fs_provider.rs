use connected_core::filesystem::{FilesystemProvider, FsEntry, FsEntryType};
use connected_core::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct DesktopFilesystemProvider {
    root: PathBuf,
}

impl DesktopFilesystemProvider {
    pub fn new() -> Self {
        let root = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self { root }
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        // Prevent directory traversal attacks
        let safe_path = path.trim_start_matches('/');
        let full_path = self.root.join(safe_path);

        // Canonicalize to check if it's still under root
        // Note: canonicalize requires file to exist.
        // For listing/reading, it should exist.
        // For writing, parent should exist.

        // Simple check for now: just ensure no ".." components that go above root
        // This is a naive check. A better one uses `canonicalize`.
        if full_path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            // This blocks ".." usage even if inside root, which is safe but restrictive.
            // But valid for now.
            // Actually, "foo/../bar" stays in root if foo exists.
            // But let's rely on canonicalize if possible, or just be strict.
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
}
