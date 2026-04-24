use std::fs;
use std::path::Path;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSource {
    Generated,
    File,
    Directory,
}

impl InputSource {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Generated => "generated",
            Self::File => "file",
            Self::Directory => "directory",
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct LoadedInput {
    pub bytes: Vec<u8>,
    pub pages: Vec<Vec<u8>>,
    pub source: InputSource,
}

pub fn load_pages_from_dir(path: &Path, page_size: usize) -> Result<Vec<Vec<u8>>, String> {
    let mut entries = fs::read_dir(path)
        .map_err(|err| format!("failed to read input dir {}: {err}", path.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to read input dir {}: {err}", path.display()))?;

    entries.sort_by_key(|left| left.path());

    let mut pages = Vec::new();
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|err| format!("failed to stat {}: {err}", entry.path().display()))?;
        if !file_type.is_file() {
            continue;
        }
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("bin") {
            continue;
        }

        let bytes = fs::read(entry.path()).map_err(|err| {
            format!(
                "failed to read input page {}: {err}",
                entry.path().display()
            )
        })?;
        if bytes.len() > page_size {
            return Err(format!(
                "input page {} has {} bytes which exceeds --page-size {}",
                entry.path().display(),
                bytes.len(),
                page_size
            ));
        }
        pages.push(bytes);
    }

    if pages.is_empty() {
        return Err(format!(
            "input dir {} does not contain any *.bin pages",
            path.display()
        ));
    }

    Ok(pages)
}

#[allow(dead_code)]
pub fn flatten_pages(pages: &[Vec<u8>]) -> Vec<u8> {
    let total_len = pages.iter().map(Vec::len).sum();
    let mut bytes = Vec::with_capacity(total_len);
    for page in pages {
        bytes.extend_from_slice(page);
    }
    bytes
}
