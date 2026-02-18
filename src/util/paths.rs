use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use directories::BaseDirs;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub fn default_data_dir() -> Result<PathBuf> {
    let base_dirs = BaseDirs::new().ok_or_else(|| anyhow!("unable to determine home directory"))?;
    Ok(base_dirs.home_dir().join(".lasersell"))
}

pub fn default_config_path() -> Result<PathBuf> {
    Ok(default_data_dir()?.join("config.yml"))
}

pub fn default_error_log_path() -> Result<PathBuf> {
    Ok(default_data_dir()?.join("error.log"))
}

#[cfg(feature = "devnet")]
pub fn default_debug_log_path() -> Result<PathBuf> {
    Ok(default_data_dir()?.join("debug.log"))
}

pub fn ensure_data_dir_exists() -> Result<()> {
    let dir = default_data_dir()?;
    let existed = dir.exists();
    fs::create_dir_all(&dir).with_context(|| format!("create data dir {}", dir.display()))?;
    #[cfg(unix)]
    if !existed {
        let perms = fs::Permissions::from_mode(0o700);
        let _ = fs::set_permissions(&dir, perms);
    }
    Ok(())
}
