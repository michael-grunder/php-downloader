// Clippy:
#![warn(clippy::all, clippy::nursery, clippy::pedantic)]
#![allow(clippy::non_ascii_literal)]

mod downloads;

use anyhow::Result;
use clap::Parser;
use colored::Colorize;
use num_format::{Locale, ToFormattedString};
use std::path::PathBuf;
use tempfile::NamedTempFile;

use crate::downloads::{DownloadList, DownloadUrl, Extension, Version};

#[derive(Parser, Debug)]
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

    #[arg(default_value = ".")]
    output_path: Option<PathBuf>,
}

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

fn print_download_urls(urls: &[DownloadUrl]) {
    // Calculating the maximum lengths of each field in a more idiomatic way
    let max_lens = urls.iter().fold([0, 0, 0, 0], |mut acc, url| {
        acc[0] = acc[0].max(url.version.to_string().len());
        acc[1] = acc[1].max(url.url.len());
        acc[2] = acc[2].max(url.size.to_human_size().len());
        acc[3] = acc[3].max(url.age.len());
        acc
    });

    // Printing each url with fields aligned based on their maximum lengths
    for url in urls {
        println!(
            "{:<width0$} {} {:<width1$} {:>width2$} {:>width3$}",
            url.version.to_string().bold(),
            "->".green(),
            url.url,
            url.size.to_human_size(),
            url.age.dimmed(),
            width0 = max_lens[0],
            width1 = max_lens[1],
            width2 = max_lens[2],
            width3 = max_lens[3],
        );
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt: Options = Options::parse();

    if opt.latest {
        let mut urls = vec![];

        let versions = opt.version.map_or_else(
            || vec![(7, 4), (8, 0), (8, 1), (8, 2)],
            |v| vec![(v.major, v.minor)],
        );

        for (major, minor) in versions {
            let downloads = DownloadList::new(major, minor, opt.extension);
            let latest = downloads.latest().await?;
            if let Some(latest) = latest {
                urls.push(latest);
            }
        }

        print_download_urls(&urls);
        std::process::exit(0);
    } else if opt.list {
        let version = opt.version.unwrap_or_else(|| Version::new(8, 2, 0));
        let urls = DownloadList::new(version.major, version.minor, opt.extension)
            .list()
            .await?;

        print_download_urls(&urls);
        std::process::exit(0);
    } else {
        let version = opt.version.unwrap_or_else(|| Version::new(8, 2, 0));
        let downloads = DownloadList::new(version.major, version.minor, opt.extension);
        version.resolve_patch(&downloads).await?;

        let dl = downloads
            .get(version.patch)
            .await?
            .expect("TODO: Error message");

        let mut tmp = NamedTempFile::new()?;
        dl.download(tmp.as_file_mut()).await?;

        let mut dst = opt.output_path.unwrap();
        dst.push(dl.version.get_file_name(opt.extension));
        tmp.persist(&dst)?;
    }

    std::process::exit(0);
}
