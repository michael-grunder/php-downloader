// Clippy:
#![warn(clippy::all, clippy::nursery, clippy::pedantic)]
#![allow(clippy::non_ascii_literal)]

mod config;
mod downloads;
mod extract;
mod hooks;
mod view;

use crate::{
    config::Config,
    downloads::{DownloadInfo, DownloadList, Extension, Version},
    extract::{BuildRoot, Tarball},
    hooks::{Hook, ScriptResult},
    view::Viewer,
};
use anyhow::{bail, Context, Result};
use clap::Parser;
use colored::Colorize;
use std::{
    fmt, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    str,
};
use tempfile::NamedTempFile;

const NEW_MAJOR: u8 = 8;
const NEW_MINOR: u8 = 2;

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

    #[arg(short, long)]
    no_hooks: bool,

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
    Upgrade,
}

macro_rules! operation_variants {
    ($($variant:ident),+) => {
        vec![
            $(
                (Self::$variant.as_str(), Self::$variant),
            )+
        ]
    };
}

impl Operation {
    fn variants() -> Vec<(&'static str, Self)> {
        operation_variants!(Cached, Download, Extract, Latest, List, Upgrade)
    }

    fn matching_operations_msg(matches: &[(&'static str, Self)]) -> String {
        matches
            .iter()
            .map(|(m, _)| format!("  {m}").bold().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn all_operations_msg() -> String {
        Self::variants()
            .into_iter()
            .map(|(op, _)| format!("  {op}").bold().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Cached => "cached",
            Self::Download => "download",
            Self::Extract => "extract",
            Self::Latest => "latest",
            Self::List => "list",
            Self::Upgrade => "upgrade",
        }
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
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
            [] => bail!(
                "Unknown operation.\n\nValid operations\n{}",
                Self::all_operations_msg()
            ),
            [(_, operation)] => Ok(*operation),
            matches => bail!(
                "Ambiguous operation.\n\nMatching operations:\n{}",
                Self::matching_operations_msg(matches)
            ),
        }
    }
}

fn validate_hook(hook: Hook, res: &ScriptResult) -> Result<()> {
    if res.status != 0 {
        let path = res.save()?;
        eprintln!("Warning:  Could not execute {hook} script.  Script output logged to '{path:?}'");
        bail!("Failed to execute hook");
    }

    Ok(())
}

// Download a specific resolved version if we don't have it
async fn get_version(version: Version, extension: Extension) -> Result<()> {
    if Tarball::latest(&Config::registry_path()?, version)?.is_none() {
        eprintln!("Unable to find {version} locally, downloading.");
        let downloads = DownloadList::new(version.major, version.minor, extension);
        let dl = downloads
            .get(version)
            .await?
            .context("Unable to get download URL for PHP {version}")?;

        let mut dst = PathBuf::from(&Config::registry_path()?);
        dst.push(version.get_file_name(extension));

        download_file(&dst, &dl).await?;
    }

    Ok(())
}

async fn op_extract(
    version: Version,
    extension: Extension,
    dst_path: &Path,
    dst_file: Option<&Path>,
    no_hooks: bool,
) -> Result<PathBuf> {
    get_version(version, extension).await?;

    let tarball = Tarball::new(version, extension)?;

    // Extract the arghive and capture full destination path
    let extracted_path = tarball
        .extract(dst_path, dst_file)?
        .canonicalize()?
        .to_string_lossy()
        .into_owned();

    if !no_hooks {
        for hook in [Hook::PostExtract, Hook::Configure, Hook::Make] {
            let res = Hook::exec(hook, &*extracted_path, &[&extracted_path])?;
            validate_hook(hook, &res)?;
        }
    }

    let root = BuildRoot::from_path(&extracted_path)?;
    let (loc, files) = root.save_manifest()?;
    eprintln!("Saved manifest {loc:?} with {files} files.");

    Ok(extracted_path.into())
}

fn op_cached(version: Option<Version>, viewer: &(dyn Viewer + Send)) -> Result<()> {
    let mut tarballs: Vec<_> = Tarball::list(&Config::registry_path()?)?
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
        let latest = DownloadList::new(major, minor, extension).latest().await?;
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
        eprintln!("{version}\t{dst:?}");
    } else {
        let dl = downloads
            .get(version)
            .await?
            .context("Unable to get download URL for PHP {version}")?;

        download_file(&dst, &dl).await?;
    }

    Ok(())
}

async fn op_upgrade_root(
    root: &BuildRoot,
    extension: Extension,
    no_hooks: bool,
) -> Result<BuildRoot> {
    let latest = DownloadList::new(root.version.major, root.version.minor, extension)
        .latest()
        .await?
        .context("Can't find latest version")?;

    if latest.version > root.version {
        eprintln!("Upgrading {} to {}", root.version, latest.version);
    }

    let mut extracted_path = op_extract(
        latest.version,
        extension,
        &root.parent(),
        Some(&PathBuf::from(root.version_path_name(latest.version))),
        no_hooks,
    )
    .await?;

    let res = BuildRoot::from_path(&extracted_path)?;

    let backup_path = root
        .src
        .file_name()
        .expect("No file name?")
        .to_string_lossy();

    extracted_path.push(format!("{}-backup-scripts", &*backup_path));

    eprintln!("Backing up scripts from old build tree...");
    root.save_scripts(&extracted_path)?;

    Ok(res)
}

fn user_confirm(msg: &str) -> Result<bool> {
    eprint!("{msg}? (yes/no)");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    Ok(input.chars().next().map_or(false, |c| c == 'y' || c == 'Y'))
}

async fn op_upgrade(path: &Path, extension: Extension, no_hooks: bool) -> Result<()> {
    let mut roots = match BuildRoot::from_path(path) {
        Ok(root) => vec![root],
        _ => BuildRoot::from_parent_path(path)?,
    };

    roots.sort_unstable();

    if roots.is_empty() {
        eprintln!("Faiiled to determine build root(s) from path {path:?}");
        return Ok(());
    }

    let mut upgrades = vec![];

    for (n, root) in roots.into_iter().enumerate() {
        eprintln!("[{}] Upgrading {:?}", 1 + n, root.src);
        let res = op_upgrade_root(&root, extension, no_hooks).await?;
        upgrades.push((root, res));
    }

    for (n, (old, new)) in upgrades.iter().enumerate() {
        eprintln!("[{}] {:?} -> {:?}", n + 1, &old.src, &new.src);
    }

    if user_confirm("Remove old path(s)")? {
        for (root, _) in upgrades {
            eprint!("Removing {:?}...", &root.src);
            root.remove()?;
            eprintln!("done!");
        }
    }

    Ok(())
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
            let path = opt
                .output_path
                .clone()
                .context("Must pass destination path")?;

            let version = opt
                .version
                .context("Please pass at least a major and minor version")?;

            Tarball::validate_writable_directory(&path)?;

            op_extract(
                version,
                opt.extension,
                &path,
                opt.output_file.as_deref(),
                opt.no_hooks,
            )
            .await?;
        }
        Operation::Latest => {
            op_latest(opt.version, opt.extension, &*viewer).await?;
        }
        Operation::List => {
            op_list(opt.version, opt.extension, &*viewer).await?;
        }
        Operation::Download => {
            let version = opt
                .version
                .context("Please pass at least a major and minor version")?;

            let path = opt.output_path.unwrap_or(Config::registry_path()?);
            op_download(version, &path, opt.extension, opt.force).await?;
        }
        Operation::Upgrade => {
            let path = opt
                .output_path
                .context("Must pass an existing build tree path!")?;

            op_upgrade(&path, opt.extension, opt.no_hooks).await?;
        }
    }

    Ok(())
}
