use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use rand::Rng;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub fn atomic_write(path: &Path, contents: &[u8], mode: Option<u32>) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("tmp");
    let tmp_name = format!(".{file_name}.tmp-{}", rand::thread_rng().gen::<u64>());
    let tmp_path = parent.join(tmp_name);

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if let Some(mode) = mode {
        options.mode(mode);
    }

    let mut file = options
        .open(&tmp_path)
        .with_context(|| format!("open temp file {}", tmp_path.display()))?;
    file.write_all(contents)
        .with_context(|| format!("write temp file {}", tmp_path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync temp file {}", tmp_path.display()))?;
    drop(file);

    #[cfg(unix)]
    if let Some(mode) = mode {
        let perms = fs::Permissions::from_mode(mode);
        let _ = fs::set_permissions(&tmp_path, perms);
    }

    fs::rename(&tmp_path, path).with_context(|| format!("replace file {}", path.display()))?;
    Ok(())
}
