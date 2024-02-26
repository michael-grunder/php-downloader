// Clippy:
#![warn(clippy::all, clippy::nursery, clippy::pedantic)]
#![allow(clippy::non_ascii_literal)]
#![allow(clippy::must_use_candidate)]

mod config;
pub mod downloads;
mod extract;
mod hooks;
mod view;

use crate::{
    config::Config,
    downloads::{DownloadList, Extension, Version},
    extract::{BuildRoot, Tarball},
    hooks::{Hook, ScriptResult},
    view::Viewer,
};
use anyhow::{bail, Context, Result};
use clap::Parser;
use std::{
    fmt,
    path::{Path, PathBuf},
    str,
};

const NEW_MAJOR: u8 = 8;
const NEW_MINOR: u8 = 2;

#[derive(Parser, Debug)]
struct Options {
    #[arg(short, long, default_value = "bz2")]
    extension: Extension,

    #[arg(short, long)]
    json: bool,

    #[arg(short, long)]
    force: bool,

    #[arg(short, long)]
    no_hooks: bool,

    #[clap(subcommand)]
    operation: Operation,
}

#[derive(Parser, Debug, Clone)]
enum Operation {
    Cached {
        version: Option<Version>,
    },
    Download {
        version: Version,
        output_path: Option<PathBuf>,
    },
    Extract {
        version: Version,

        #[clap(value_parser = is_writable_dir)]
        output_path: PathBuf,

        output_file: Option<PathBuf>,
    },
    Latest {
        version: Option<Version>,
    },
    List {
        version: Option<Version>,
    },
    Upgrade {
        path: PathBuf,
    },
    Version,
}

impl Operation {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Cached { .. } => "cached",
            Self::Download { .. } => "download",
            Self::Extract { .. } => "extract",
            Self::Latest { .. } => "latest",
            Self::List { .. } => "list",
            Self::Upgrade { .. } => "upgrade",
            Self::Version => "version",
        }
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
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

async fn op_extract(
    mut version: Version,
    extension: Extension,
    dst_path: &Path,
    dst_file: Option<&Path>,
    no_hooks: bool,
) -> Result<PathBuf> {
    // If we only have major.minor just resolve patch if we can
    let downloads = DownloadList::new(version.major, version.minor, extension);
    version.resolve_latest(&downloads).await?;

    let tarball = Tarball::get_or_download(version, extension).await?;

    if let Some(path) = tarball.check_dst_path(dst_path, dst_file)? {
        return Err(anyhow::anyhow!("Path {path:?} already exists"));
    }

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
            .context(format!("Unable to get download URL for PHP {version}"))?;

        dl.download_to_file(&dst).await?;
    }

    Ok(())
}

async fn op_upgrade_root(
    root: &BuildRoot,
    extension: Extension,
    no_hooks: bool,
) -> Result<Option<BuildRoot>> {
    let latest = DownloadList::new(root.version.major, root.version.minor, extension)
        .latest()
        .await?
        .context("Can't find latest version")?;

    if latest.version > root.version {
        eprintln!("    {} -> {}", root.version, latest.version);
    } else {
        eprintln!(
            "    Version {} is already the latest version, skipping.",
            root.version
        );
        return Ok(None);
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
    if let Err(e) = root.save_scripts(&extracted_path) {
        eprintln!("Warning:  Unable to backup new scripts ({e:?})");
    }

    Ok(Some(res))
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
        match op_upgrade_root(&root, extension, no_hooks).await {
            Ok(Some(res)) => upgrades.push((root, res)),
            Err(e) => eprintln!("    Warning: {e:?}"),
            _ => {}
        }
    }

    for (n, (old, new)) in upgrades.iter().enumerate() {
        eprintln!("[{}] {:?} -> {:?}", n + 1, &old.src, &new.src);
    }

    if !upgrades.is_empty() && user_confirm("Remove old path(s)")? {
        for (root, _) in upgrades {
            eprint!("Removing {:?}...", &root.src);
            root.remove()?;
            eprintln!("done!");
        }
    }

    Ok(())
}

fn is_writable_dir(s: &str) -> std::result::Result<PathBuf, String> {
    let path = Path::new(s);

    if !path.is_dir() {
        Err(format!("'{s} is not a directory!"))
    } else if std::fs::metadata(path)
        .map_err(|e| e.to_string())?
        .permissions()
        .readonly()
    {
        Err(format!("The directory '{s}' is not writable"))
    } else {
        Ok(PathBuf::from(path))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt: Options = Options::parse();

    let viewer = view::get_viewer(opt.json);

    match opt.operation {
        Operation::Cached { version } => {
            op_cached(version, &*viewer)?;
        }
        Operation::Extract {
            version,
            output_path,
            output_file,
        } => {
            op_extract(
                version,
                opt.extension,
                &output_path,
                output_file.as_deref(),
                opt.no_hooks,
            )
            .await?;
        }
        Operation::Latest { version } => {
            op_latest(version, opt.extension, &*viewer).await?;
        }
        Operation::List { version } => {
            op_list(version, opt.extension, &*viewer).await?;
        }
        Operation::Download {
            version,
            output_path,
        } => {
            let path = output_path.unwrap_or(Config::registry_path()?);
            op_download(version, &path, opt.extension, opt.force).await?;
        }
        Operation::Upgrade { path } => {
            op_upgrade(&path, opt.extension, opt.no_hooks).await?;
        }
        Operation::Version => {
            println!("{} {}", env!("CARGO_BIN_NAME"), env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
    }

    Ok(())
}
