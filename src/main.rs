// Clippy:
#![warn(clippy::all, clippy::nursery, clippy::pedantic)]
#![allow(clippy::non_ascii_literal)]

mod downloads;

use crate::downloads::{DownloadList, DownloadUrl, Extension, Version};
use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use serde_json::to_string_pretty;
use std::path::PathBuf;
use tempfile::NamedTempFile;

const NEW_MAJOR: u8 = 8;
const NEW_MINOR: u8 = 2;

#[derive(Parser, Debug)]
#[allow(clippy::struct_excessive_bools)]
struct Options {
    #[arg(short, long)]
    list: bool,

    #[arg(short, long)]
    verify: bool,

    #[arg(short, long, default_value = "bz2")]
    extension: Extension,

    version: Option<Version>,

    #[arg(long)]
    latest: bool,

    #[arg(short, long)]
    json: bool,

    #[arg(default_value = ".")]
    output_path: Option<PathBuf>,
}

trait Viewer {
    fn display(&self, data: &[DownloadUrl]);
}

struct CliViewer;
struct JsonViewer;

impl<T: Into<u64>> ToHumanSize for T {
    fn to_human_size(self) -> String {
        Self::to_human_size_fmt(self.into())
    }
}

trait ToHumanSize {
    fn to_human_size(self) -> String;

    fn to_human_size_fmt(v: u64) -> String {
        let (val, unit) = Self::to_human_size_impl(v);
        format!("{val:.2} {unit}")
    }

    fn to_human_size_impl(v: u64) -> (f64, &'static str) {
        const KB: f64 = 1024.0;
        const MB: f64 = KB * 1024.0;
        const GB: f64 = MB * 1024.0;
        const TB: f64 = GB * 1024.0;

        #[allow(clippy::cast_precision_loss)]
        let v = v as f64;

        if v < KB {
            (v, "B")
        } else if v < MB {
            (v / KB, "KB")
        } else if v < GB {
            (v / MB, "MB")
        } else if v < TB {
            (v / GB, "GB")
        } else {
            (v / TB, "TB")
        }
    }
}

impl Viewer for CliViewer {
    fn display(&self, urls: &[DownloadUrl]) {
        // Calculating the maximum lengths of each field in a more idiomatic way
        let max_lens = urls.iter().fold([0, 0, 0, 0], |mut acc, url| {
            acc[0] = acc[0].max(url.version.to_string().len());
            acc[1] = acc[1].max(url.size.to_human_size().len());
            acc[2] = acc[2].max(url.date_string().len());
            acc[3] = acc[3].max(url.url.len());
            acc
        });

        // Printing each url with fields aligned based on their maximum lengths
        // "{:<width0$} \u{2502} {:<width1$} {:>width2$} \u{2192} {:<width3$}",
        for url in urls {
            println!(
                "{:<width0$} {:<width1$} {:>width2$} | {:<width3$}",
                url.version.to_string().bold(),
                url.size.to_human_size(),
                url.date_string(),
                url.url,
                width0 = max_lens[0],
                width1 = max_lens[1],
                width2 = max_lens[2],
                width3 = max_lens[3],
            );
        }
    }
}

impl Viewer for JsonViewer {
    fn display(&self, urls: &[DownloadUrl]) {
        let s = to_string_pretty(urls).unwrap_or_else(|_| String::from("Error generating JSON"));
        println!("{s}");
    }
}

fn viewer(json: bool) -> Box<dyn Viewer> {
    if json {
        Box::new(JsonViewer)
    } else {
        Box::new(CliViewer)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt: Options = Options::parse();

    if opt.latest {
        let versions = opt.version.map_or_else(
            || vec![(7, 4), (8, 0), (8, 1), (8, 2), (8, 3)],
            |v| vec![(v.major, v.minor)],
        );

        let mut urls = vec![];

        for (major, minor) in versions {
            let downloads = DownloadList::new(major, minor, opt.extension);
            let latest = downloads.latest().await?;
            if let Some(latest) = latest {
                urls.push(latest);
            }
        }

        viewer(opt.json).display(&urls);
    } else if opt.list {
        let version = opt
            .version
            .unwrap_or_else(|| Version::from_major_minor(NEW_MAJOR, NEW_MINOR));
        let urls = DownloadList::new(version.major, version.minor, opt.extension)
            .list()
            .await?;

        viewer(opt.json).display(&urls);
    } else {
        let mut version = opt
            .version
            .context("Please pass at least a major and minor version to download")?;

        let downloads = DownloadList::new(version.major, version.minor, opt.extension);
        version.resolve_latest(&downloads).await?;

        let dl = downloads
            .get(version)
            .await?
            .context("Unable to get download URL for PHP {version}")?;

        let mut tmp = NamedTempFile::new()?;
        dl.download(tmp.as_file_mut()).await?;

        let mut dst = opt.output_path.unwrap();
        dst.push(version.get_file_name(opt.extension));
        tmp.persist(&dst)?;
    }

    std::process::exit(0);
}
