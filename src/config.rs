use anyhow::{Context, Result};
use std::path::PathBuf;

const APP_CFG_PATH: &str = ".phpfarm";
const APP_REGISTRY_PATH: &str = "tarballs";
const APP_HOOKS_PATH: &str = "hooks";

pub struct Config;

impl Config {
    fn get_base_app_path() -> Result<PathBuf> {
        let v = if let Ok(path) = std::env::var("PHPFARM_ROOT") {
            path
        } else if cfg!(windows) {
            std::env::var("USERPROFILE")?
        } else {
            std::env::var("HOME")?
        };

        Ok(PathBuf::from(v))
    }

    fn app_path<S: AsRef<str>>(child: Option<S>) -> Result<PathBuf> {
        let mut dir = Self::get_base_app_path()?;
        dir.push(APP_CFG_PATH);

        if let Some(child) = child {
            dir.push(child.as_ref());
        }

        std::fs::create_dir_all(&dir).context(format!("Unable to create directory '{dir:?}'"))?;

        Ok(dir)
    }

    pub fn registry_path() -> Result<PathBuf> {
        Self::app_path(Some(APP_REGISTRY_PATH))
    }

    pub fn hooks_path() -> Result<PathBuf> {
        Self::app_path(Some(APP_HOOKS_PATH))
    }
}
