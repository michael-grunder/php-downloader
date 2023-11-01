// Clippy:
#![warn(clippy::all, clippy::nursery, clippy::pedantic)]
#![allow(clippy::non_ascii_literal)]

mod downloads;
mod extract;
mod hooks;
mod view;

use crate::{
    downloads::{DownloadInfo, DownloadList, Extension, Version},
    extract::{Extract, Tarball},
    hooks::Hook,
    view::Viewer,
};
use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::{
    fmt, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    str,
};
use tempfile::NamedTempFile;

const NEW_MAJOR: u8 = 8;
const NEW_MINOR: u8 = 2;

const APP_CFG_PATH: &str = ".phpfarm";
const APP_REGISTRY_PATH: &str = "tarballs";
const APP_HOOKS_PATH: &str = "hooks";

#[derive(Parser, Debug)]
struct Options {
    operation: Operation,

    version: Option<Version>,

    #[arg(short, long, default_value = "bz2")]
    extension: Extension,

    #[arg(short, long)]
    json: bool,

    #[arg(short, long)]
    force: bool,

    output_path: Option<PathBuf>,
    output_file: Option<PathBuf>,
}

#[derive(Debug, Copy, Clone)]
enum Operation {
    Cached,
    Download,
    Extract,
    Latest,
    List,
}

impl Operation {
    fn variants() -> Vec<(&'static str, Self)> {
        vec![
            ("cached", Self::Cached),
            ("download", Self::Download),
            ("extract", Self::Extract),
            ("latest", Self::Latest),
            ("list", Self::List),
        ]
    }

    fn possible_matches_msg(matches: &[(&'static str, Self)]) -> String {
        matches
            .iter()
            .map(|(m, _)| (*m).to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Cached => "cached",
                Self::Download => "download",
                Self::Extract => "extract",
                Self::Latest => "latest",
                Self::List => "list",
            },
        )
    }
}

impl str::FromStr for Operation {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let matches: Vec<_> = Self::variants()
            .into_iter()
            .filter(|(name, _)| name.to_lowercase().starts_with(&s.to_lowercase()))
            .collect();

        match matches.as_slice() {
            [] => Err(anyhow!("No matching operation")),
            [(_, operation)] => Ok(*operation),
            matches => Err(anyhow!(
                "Ambiguous operation. Matches: {:?}",
                Self::possible_matches_msg(matches)
            )),
        }
    }
}

fn op_extract(version: Version, dst_path: &Path, dst_file: Option<&Path>) -> Result<()> {
    let tarball: Tarball = Tarball::latest(&registry_path()?, version)
        .transpose()
        .context(format!("Unable to find a tarball for version '{version}'",))??
        .into();

    println!("Found tarball {tarball:?}");

    let full_dst_path = tarball.extract(dst_path, dst_file)?;

    Hook::exec(
        Hook::PostExtract,
        &[&full_dst_path.canonicalize()?.to_string_lossy()],
    )
}

fn op_cached(version: Option<Version>, viewer: &(dyn Viewer + Send)) -> Result<()> {
    let mut tarballs: Vec<_> = Tarball::list(&registry_path()?)?
        .into_iter()
        .filter(|fi| fi.version.optional_matches(version))
        .collect();

    tarballs.sort_by(|a, b| a.version.cmp(&b.version));

    viewer.display(&tarballs);

    Ok(())
}

async fn op_latest(
    version: Option<Version>,
    extension: Extension,
    viewer: &(dyn Viewer + Send),
) -> Result<()> {
    let versions = version.map_or_else(
        || vec![(7, 4), (8, 0), (8, 1), (8, 2), (8, 3)],
        |v| vec![(v.major, v.minor)],
    );

    let mut urls = vec![];

    for (major, minor) in versions {
        let downloads = DownloadList::new(major, minor, extension);
        let latest = downloads.latest().await?;
        if let Some(latest) = latest {
            urls.push(latest);
        }
    }

    viewer.display(&urls);

    Ok(())
}

async fn op_list(
    version: Option<Version>,
    extension: Extension,
    viewer: &(dyn Viewer + Send),
) -> Result<()> {
    let version = version.unwrap_or_else(|| Version::from_major_minor(NEW_MAJOR, NEW_MINOR));
    let urls = DownloadList::new(version.major, version.minor, extension)
        .list()
        .await?;

    viewer.display(&urls);

    Ok(())
}

async fn download_file(dst: &Path, dl: &DownloadInfo) -> Result<()> {
    let mut tmp = NamedTempFile::new()?;

    let mut perms = fs::metadata(tmp.path())?.permissions();
    perms.set_mode(0o644);
    fs::set_permissions(tmp.path(), perms)?;

    dl.download(tmp.as_file_mut()).await?;

    tmp.persist(dst)?;

    Ok(())
}

async fn op_download(
    mut version: Version,
    path: &Path,
    extension: Extension,
    overwrite: bool,
) -> Result<()> {
    let downloads = DownloadList::new(version.major, version.minor, extension);

    // Resolve to the actual major.minor.patch (if needed)
    version.resolve_latest(&downloads).await?;

    let mut dst = PathBuf::from(path);
    dst.push(version.get_file_name(extension));

    if !overwrite && dst.exists() {
        println!("{version} -> {dst:?}");
    } else {
        let dl = downloads
            .get(version)
            .await?
            .context("Unable to get download URL for PHP {version}")?;

        download_file(&dst, &dl).await?;
    }

    Ok(())
}

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
    let mut dir = get_base_app_path()?;
    dir.push(APP_CFG_PATH);

    if let Some(child) = child {
        dir.push(child.as_ref());
    }

    std::fs::create_dir_all(&dir).context(format!("Unable to create directory '{dir:?}'"))?;

    Ok(dir)
}

fn registry_path() -> Result<PathBuf> {
    app_path(Some(APP_REGISTRY_PATH))
}

fn hooks_path() -> Result<PathBuf> {
    app_path(Some(APP_HOOKS_PATH))
}

fn required_version(version: Option<Version>) -> Result<Version> {
    version.context("Please pass at least a major and minor version to download")
}

fn validate_output_path(path: &Option<PathBuf>) -> Result<PathBuf> {
    let path = path.clone().context("Missing destination path")?;

    if !path.exists() {
        return Err(anyhow!("Path does not exist: {}", path.display()));
    }

    if !path.is_dir() {
        return Err(anyhow!("Path is not a directory: {}", path.display()));
    }

    if fs::metadata(&path)?.permissions().readonly() {
        return Err(anyhow!("Path is not writable: {}", path.display()));
    }

    Ok(path)
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt: Options = Options::parse();

    let viewer = view::get_viewer(opt.json);

    match opt.operation {
        Operation::Cached => {
            op_cached(opt.version, &*viewer)?;
        }
        Operation::Extract => {
            op_extract(
                required_version(opt.version)?,
                &validate_output_path(&opt.output_path)?,
                opt.output_file.as_deref(),
            )?;
        }
        Operation::Latest => {
            op_latest(opt.version, opt.extension, &*viewer).await?;
        }
        Operation::List => {
            op_list(opt.version, opt.extension, &*viewer).await?;
        }
        Operation::Download => {
            let version = required_version(opt.version)?;
            let path = opt.output_path.unwrap_or(registry_path()?);
            op_download(version, &path, opt.extension, opt.force).await?;
        }
    }

    Ok(())
}
