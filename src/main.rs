// Clippy:
#![warn(clippy::all, clippy::nursery, clippy::pedantic)]
#![allow(clippy::non_ascii_literal)]

mod downloads;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use colored::Colorize;
use tempfile::NamedTempFile;
//use downloader::Downloader;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use num_format::{Locale, ToFormattedString};
use std::{fmt, path::PathBuf};

use crate::downloads::{DownloadList, DownloadUrl, Extension, Version};

#[derive(Parser, Debug)]
struct Options {
    #[arg(short, long)]
    list: bool,

    #[arg(short, long)]
    verify: bool,

    #[arg(short, long, default_value = "bz2")]
    extension: Extension,

    #[structopt(default_value = "8.2.0")]
    version: Version,

    #[arg(long)]
    latest: bool,

    #[arg(default_value = ".")]
    output_path: Option<PathBuf>,
}

fn print_download_urls(urls: &[DownloadUrl]) {
    // Calculating the maximum lengths of each field in a more idiomatic way
    let max_lens = urls.iter().fold([0, 0, 0, 0], |mut acc, url| {
        acc[0] = acc[0].max(url.version.to_string().len());
        acc[1] = acc[1].max(url.url.len());
        acc[2] = acc[2].max(url.size.to_formatted_string(&Locale::en).len());
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
            url.size.to_formatted_string(&Locale::en),
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

        for (major, minor) in [(7, 4), (8, 0), (8, 1), (8, 2)] {
            let downloads = DownloadList::new(major, minor, opt.extension);
            let latest = downloads.latest().await?;
            if let Some(latest) = latest {
                urls.push(latest);
            }
        }

        print_download_urls(&urls);
        std::process::exit(0);
    } else if opt.list {
        let urls = DownloadList::new(opt.version.major, opt.version.minor, opt.extension)
            .list()
            .await?;

        print_download_urls(&urls);
        std::process::exit(0);
    } else {
        let downloads = DownloadList::new(opt.version.major, opt.version.minor, opt.extension);

        let version = Version::new(
            opt.version.major,
            opt.version.minor,
            if opt.version.had_patch {
                opt.version.patch
            } else {
                downloads.latest().await?.unwrap().version.patch
            },
        );

        let mut tmp = NamedTempFile::new()?;
        downloads.download(version.patch, tmp.as_file_mut()).await?;

        let mut dst = opt.output_path.unwrap();
        dst.push(version.get_file_name(opt.extension));
        tmp.persist(&dst)?;
    }

    std::process::exit(0);
}
