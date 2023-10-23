// Clippy:
#![warn(clippy::all, clippy::nursery, clippy::pedantic)]
#![allow(clippy::non_ascii_literal)]

mod downloads;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use colored::Colorize;
use downloader::Downloader;
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

struct ProgressBarContainer {
    progress_bar: ProgressBar,
    data_len: u64,
}

// Define a custom progress reporter:
struct SimpleReporterPrivate {
    progress_bar: Option<ProgressBarContainer>,
}

struct SimpleReporter {
    private: std::sync::Mutex<Option<SimpleReporterPrivate>>,
}

impl SimpleReporter {
    #[cfg(not(feature = "tui"))]
    fn create() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            private: std::sync::Mutex::new(None),
        })
    }
}

const PB_TEMPLATE: &str =
    "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})";

impl ProgressBarContainer {
    fn progress_bar(max: u64) -> ProgressBar {
        let progress_bar = ProgressBar::new(max);
        progress_bar.set_style(
            ProgressStyle::with_template(PB_TEMPLATE)
                .unwrap()
                .with_key("eta", |state: &ProgressState, w: &mut dyn fmt::Write| {
                    write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap();
                })
                .progress_chars("#>-"),
        );

        progress_bar
    }

    pub fn new(data_len: u64) -> Self {
        Self {
            progress_bar: Self::progress_bar(data_len),
            data_len,
        }
    }
}

impl downloader::progress::Reporter for SimpleReporter {
    fn setup(&self, max_progress: Option<u64>, _: &str) {
        let max_progress = max_progress.unwrap_or(0);

        let progress_bar = if max_progress > 4096 {
            Some(ProgressBarContainer::new(max_progress))
        } else {
            None
        };

        let private = SimpleReporterPrivate { progress_bar };

        let mut guard = self.private.lock().unwrap();
        *guard = Some(private);
    }

    fn progress(&self, current: u64) {
        if let Some(p) = self.private.lock().unwrap().as_mut() {
            if let Some(pb) = &p.progress_bar {
                pb.progress_bar.set_position(current);
            }
        }
    }

    fn set_message(&self, _: &str) {}

    fn done(&self) {
        if let Some(p) = self.private.lock().unwrap().as_mut() {
            if let Some(p) = &p.progress_bar {
                p.progress_bar.set_position(p.data_len);
            }
        }

        let mut guard = self.private.lock().unwrap();
        *guard = None;
    }
}

//fn print_download_url(du: &DownloadUrl) {
//    println!(
//        "{:>7} {} {} ({})",
//        du.version.to_string().bold(),
//        "->".green(),
//        du.url,
//        du.age
//    );
//}

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
        let mut downloader = Downloader::builder().build()?;
        let (file, url) = opt.version.get_download_tuple(opt.extension);

        let mut dl = downloader::Download::new(&url);
        dl = dl.file_name(file.path());
        dl = dl.progress(SimpleReporter::create());
        let downloads: Vec<downloader::Download> = vec![dl];

        let result = downloader.download(&downloads)?;

        if let Some(summary) = result.into_iter().flatten().next() {
            let mut path = opt.output_path.clone().unwrap();
            path.push(opt.version.get_file_name(opt.extension));
            println!("{:?} -> {path:?}", summary.file_name);
            std::fs::rename(summary.file_name, path)?;
            std::process::exit(0);
        }
    }

    //let mut downloader = Downloader::builder().parallel_requests(2).build().unwrap();
    //let mut downloads: Vec<downloader::Download> = vec![];

    //for (file, url) in opt.version.get_urls() {
    //    let mut dl = downloader::Download::new(&url);
    //    dl = dl.file_name(file.path());
    //    dl = dl.progress(SimpleReporter::create());
    //    downloads.push(dl);
    //}

    //let result = downloader.download(&downloads)?;

    //for summary in result.into_iter().flatten() {
    //    let mut path = opt.output_path.unwrap();
    //    path.push(opt.version.get_file_name());
    //    println!("{:?} -> {path:?}", summary.file_name);
    //    std::fs::rename(summary.file_name, path)?;
    //    std::process::exit(0);
    //}

    std::process::exit(1);
}
