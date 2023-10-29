// Clippy:
#![warn(clippy::all, clippy::nursery, clippy::pedantic)]
#![allow(clippy::non_ascii_literal)]

mod downloads;
mod view;

use crate::downloads::{DownloadList, Extension, Version};
use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::{
    fmt,
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

    #[arg(default_value = ".")]
    output_path: PathBuf,
}

#[derive(Debug, Copy, Clone)]
enum Operation {
    List,
    Download,
    Latest,
}

impl Operation {
    fn variants() -> Vec<(&'static str, Self)> {
        vec![
            ("list", Self::List),
            ("download", Self::Download),
            ("latest", Self::Latest),
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
                Self::List => "list",
                Self::Download => "download",
                Self::Latest => "latest",
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

async fn op_latest(versions: Option<Version>, extension: Extension, json: bool) -> Result<()> {
    let versions = versions.map_or_else(
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

    view::get_viewer(json).display(&urls);

    Ok(())
}

async fn op_list(version: Option<Version>, extension: Extension, json: bool) -> Result<()> {
    let version = version.unwrap_or_else(|| Version::from_major_minor(NEW_MAJOR, NEW_MINOR));
    let urls = DownloadList::new(version.major, version.minor, extension)
        .list()
        .await?;

    view::get_viewer(json).display(&urls);

    Ok(())
}

async fn op_download(mut version: Version, path: &Path, extension: Extension) -> Result<()> {
    let downloads = DownloadList::new(version.major, version.minor, extension);
    version.resolve_latest(&downloads).await?;

    let dl = downloads
        .get(version)
        .await?
        .context("Unable to get download URL for PHP {version}")?;

    let mut tmp = NamedTempFile::new()?;
    dl.download(tmp.as_file_mut()).await?;

    let mut dst = PathBuf::from(path);
    dst.push(version.get_file_name(extension));
    tmp.persist(&dst)?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt: Options = Options::parse();

    match opt.operation {
        Operation::Latest => {
            op_latest(opt.version, opt.extension, opt.json).await?;
        }
        Operation::List => {
            op_list(opt.version, opt.extension, opt.json).await?;
        }
        Operation::Download => {
            let version = opt
                .version
                .context("Please pass at least a major and minor version to download")?;
            op_download(version, &opt.output_path, opt.extension).await?;
        }
    }

    Ok(())
}
