use crate::downloads::Version;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub struct Config;

#[derive(Deserialize, Debug)]
struct PhpVersion {
    version: Version,
}

#[derive(Deserialize, Debug)]
struct PhpActiveReleases {
    #[serde(flatten)]
    versions: HashMap<String, HashMap<String, PhpVersion>>,
}

impl PhpActiveReleases {
    const PHP_RELEASES_URL: &'static str =
        "https://www.php.net/releases/active/";

    async fn fetch_active_versions() -> Result<Vec<Version>> {
        let client = Client::new();
        let response = client
            .get(Self::PHP_RELEASES_URL)
            .send()
            .await
            .context("Unable to fetch PHP releases")?
            .json::<Self>()
            .await
            .context("Unable to parse PHP releases")?;

        let versions = response
            .versions
            .values()
            .flat_map(|v| v.values())
            .map(|v| v.version)
            .collect::<Vec<Version>>();

        Ok(versions)
    }

    fn save_active_versions<P: AsRef<Path>>(
        path: P,
        versions: &Vec<Version>,
    ) -> Result<()> {
        let file = std::fs::File::create(path)?;
        serde_json::to_writer(file, versions)?;
        Ok(())
    }
}

impl Config {
    pub const APP_CFG_PATH: &'static str = ".phpdownloader";
    pub const APP_REGISTRY_PATH: &'static str = "tarballs";
    pub const APP_HOOKS_PATH: &'static str = "hooks";
    pub const APP_MANIFEST_FILE: &'static str = ".phpdownloader-manifest";
    pub const ACTIVE_FILE: &'static str = "active.json";

    const ACTIVE_VERSION_LIFESPAN: u64 = 60 * 60 * 24 * 7;

    fn get_base_app_path() -> Result<PathBuf> {
        let v = if let Ok(path) = std::env::var("PHPDOWNLOADER_ROOT") {
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
        dir.push(Self::APP_CFG_PATH);

        if let Some(child) = child {
            dir.push(child.as_ref());
        }

        std::fs::create_dir_all(&dir)
            .context(format!("Unable to create directory {}", dir.display()))?;

        Ok(dir)
    }

    pub fn registry_path() -> Result<PathBuf> {
        Self::app_path(Some(Self::APP_REGISTRY_PATH))
    }

    pub fn hooks_path() -> Result<PathBuf> {
        Self::app_path(Some(Self::APP_HOOKS_PATH))
    }

    fn active_version_file() -> Result<PathBuf> {
        let mut path = Self::app_path(None::<&str>)?;
        path.push(Self::ACTIVE_FILE);

        Ok(path)
    }

    fn save_active_versions(versions: &Vec<Version>) -> Result<()> {
        let file = Self::active_version_file()?;
        PhpActiveReleases::save_active_versions(file, versions)
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn have_versions<P: AsRef<Path>>(path: P, age_limit: u64) -> bool {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .and_then(|modified| {
                modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(std::io::Error::other)
            })
            .map(|duration| duration.as_secs() + age_limit > Self::now())
            .unwrap_or(false)
    }

    fn load_active_versions() -> Result<Vec<Version>> {
        if !Self::have_versions(
            Self::active_version_file()?,
            Self::ACTIVE_VERSION_LIFESPAN,
        ) {
            return Err(anyhow::anyhow!("No active versions available"));
        }

        let file = Self::active_version_file()?;
        let file = std::fs::File::open(file)?;
        let versions = serde_json::from_reader(file)?;

        Ok(versions)
    }

    async fn active_versions() -> Result<Vec<Version>> {
        if let Ok(versions) = Self::load_active_versions() {
            return Ok(versions);
        }

        let v = PhpActiveReleases::fetch_active_versions().await;

        if let Ok(versions) = &v {
            Self::save_active_versions(versions)?;
        }

        v
    }

    pub async fn active_version() -> Result<Version> {
        Self::active_versions()
            .await?
            .iter()
            .max()
            .copied()
            .context("No current version found")
    }
}
