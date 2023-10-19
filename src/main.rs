// Clippy:
#![warn(clippy::all, clippy::nursery, clippy::pedantic)]
#![allow(clippy::non_ascii_literal)]

use anyhow::anyhow;
use clap::Parser;
use colored::Colorize;
use downloader::Downloader;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use reqwest::{Client, Error};
use serde::{de, Deserialize, Deserializer};
use std::{fmt, path::PathBuf};
use tokio::task::JoinHandle;

#[derive(Debug)]
struct UrlInfo {
    version: Version,
    url: String,
    exists: bool,
}

#[derive(Debug, Clone, Default, Copy)]
struct Version {
    major: u16,
    minor: u16,
    patch: u16,
}

#[derive(Debug, Clone, Copy)]
enum Extension {
    GZ,
    BZ,
    XY,
}

#[derive(Parser, Debug)]
struct Options {
    #[arg(short, long)]
    list: bool,

    #[arg(short, long)]
    verify: bool,

    #[arg(short, long, default_value = "bz2")]
    extension: Extension,

    version: Version,

    #[arg(default_value = ".")]
    output_path: Option<PathBuf>,
}

impl std::default::Default for Extension {
    fn default() -> Self {
        Self::BZ
    }
}

impl std::string::ToString for Version {
    fn to_string(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl std::str::FromStr for Extension {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match &*s.to_lowercase() {
            "bz2" | "bz" => Ok(Self::BZ),
            "gz" => Ok(Self::GZ),
            "xy" => Ok(Self::XY),
            _ => Err(anyhow!("Unknown extension")),
        }
    }
}

impl fmt::Display for Extension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> fmt::Result {
        let ext = match self {
            Self::BZ => "bz2",
            Self::GZ => "gz",
            Self::XY => "xy",
        };

        write!(f, "{ext}")
    }
}

impl std::str::FromStr for Version {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('.').collect();

        if parts.len() != 2 && parts.len() != 3 {
            return Err(anyhow!("Invalid version string '{s}'"));
        }

        let major = parts[0]
            .parse()
            .map_err(|_| anyhow!("Invalid major version"))?;
        let minor = parts[1]
            .parse()
            .map_err(|_| anyhow!("Invalid minor version"))?;
        let patch = if parts.len() == 3 {
            parts[2]
                .parse()
                .map_err(|_| anyhow!("Invalid patch version"))?
        } else {
            0
        };

        Ok(Self::new(major, minor, patch))
    }
}

impl UrlInfo {
    pub fn new(url: &str, version: Version, exists: bool) -> Self {
        Self {
            url: url.into(),
            version,
            exists,
        }
    }
}

impl Version {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    fn get_file_name(self) -> PathBuf {
        PathBuf::from(format!(
            "php-{}.{}.{}.tar.bz2",
            self.major, self.minor, self.patch
        ))
    }

    fn get_temp_file() -> tempfile::NamedTempFile {
        tempfile::NamedTempFile::new().expect("Can't create temporary file")
    }

    fn get_urls(self) -> Vec<(tempfile::NamedTempFile, String)> {
        vec![
            (
                Self::get_temp_file(),
                format!(
                    "https://museum.php.net/php{}/{}",
                    self.major,
                    self.get_file_name().to_string_lossy(),
                ),
            ),
            (
                Self::get_temp_file(),
                format!(
                    "https://php.net/distributions/{}",
                    self.get_file_name().to_string_lossy(),
                ),
            ),
        ]
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        std::str::FromStr::from_str(&s).map_err(de::Error::custom)
    }
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

// Make list_urls2 an async function
async fn list_urls2(major: u16, minor: u16, ext: Extension) -> Result<(), Error> {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let mut tasks = vec![];

    for patch in 0..50 {
        let stop_flag = stop_flag.clone();
        let task = tokio::spawn(async move {
            if stop_flag.load(Ordering::SeqCst) {
                println!("Stop flag detected, short circuiting...");
                return Ok::<_, ()>(None);
            }

            let url =
                format!("https://php.net/distributions/php-{major}.{minor}.{patch}.tar.{ext}");
            println!("Checking {url}");

            if let Ok(exists) = check_url_exists(&url).await {
                if !exists {
                    println!("Setting stop flag...");
                    stop_flag.store(true, Ordering::SeqCst);
                }
                Ok(Some(UrlInfo::new(
                    &url,
                    Version::new(major, minor, patch),
                    exists,
                )))
            } else {
                Ok(None)
            }
        });

        tasks.push(task);
    }

    let mut urls_info = Vec::new();
    for task in tasks {
        if let Ok(Some(info)) = task.await.unwrap() {
            urls_info.push(info);
        }
    }

    //urls_info.sort(); // Ensure they are ordered by version
    for info in urls_info {
        if info.exists {
            println!("{:?}", info.url);
        }
    }

    Ok(())
}

//fn main() -> Result<(), Box<dyn std::error::Error>> {
//    let runtime = tokio::runtime::Runtime::new()?;
//    runtime.block_on(list_urls2(8, 2, Extension::BZ))?;
//
//    Ok(())
//}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(list_urls2(8, 2, Extension::BZ))?;
    std::process::exit(0);

    //    if opt.list {
    //        if opt.version.major != 7 && opt.version.major != 8 {
    //            eprintln!("Pass either 7 or 8 if you want a list of URLs");
    //            std::process::exit(1);
    //        }
    //        list_urls(opt.version.major, opt.version.minor, opt.extension);
    //        std::process::exit(0);
    //    }

    //    let mut downloader = Downloader::builder().parallel_requests(2).build().unwrap();
    //    let mut downloads: Vec<downloader::Download> = vec![];

    //    for (file, url) in opt.version.get_urls() {
    //        let mut dl = downloader::Download::new(&url);
    //        dl = dl.file_name(file.path());
    //        dl = dl.progress(SimpleReporter::create());
    //        downloads.push(dl);
    //    }
    //
    //    let result = downloader.download(&downloads)?;
    //
    //    for summary in result.into_iter().flatten() {
    //        let mut path = opt.output_path.unwrap();
    //        path.push(opt.version.get_file_name());
    //        println!("{:?} -> {path:?}", summary.file_name);
    //        std::fs::rename(summary.file_name, path)?;
    //        std::process::exit(0);
    //    }
    //
    //    std::process::exit(1);
}

async fn check_url_exists(url: &str) -> Result<bool, Error> {
    let client = Client::new();
    let response = client.head(url).send().await?;

    // Check if the status code indicates success
    Ok(response.status().is_success())
}

fn check_urls_exist(urls: Vec<(Version, String)>) -> Vec<JoinHandle<Result<UrlInfo, Error>>> {
    let mut tasks = vec![];

    for (version, url) in urls {
        let task = tokio::spawn(async move {
            println!("Checking {url}");
            let exists = check_url_exists(&url).await?;
            Ok(UrlInfo::new(&url, version, exists))
        });
        tasks.push(task);
    }

    tasks
}

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

//fn list_urls2(major: u16, minor: u16, ext: Extension) {
//    let stop_flag = Arc::new(AtomicBool::new(false));
//
//    let mut tasks = vec![];
//    for patch in 0..50 {
//        let stop_flag = stop_flag.clone();
//
//        let task = tokio::spawn(async move {
//            if stop_flag.load(Ordering::SeqCst) {
//                return Ok::<_, ()>(None);
//            }
//
//            let url =
//                format!("https://php.net/distributions/php-{major}.{minor}.{patch}.tar.{ext}");
//            println!("Checking {url}");
//
//            if let Ok(exists) = check_url_exists(&url).await {
//                if !exists {
//                    stop_flag.store(true, Ordering::SeqCst);
//                }
//                Ok(Some(UrlInfo::new(
//                    &url,
//                    Version::new(major, minor, patch),
//                    exists,
//                )))
//            } else {
//                Ok(None)
//            }
//        });
//
//        tasks.push(task);
//    }
//
//    let runtime = tokio::runtime::Runtime::new().unwrap();
//    runtime.block_on(async move {
//        let mut urls_info = Vec::new();
//
//        for task in tasks {
//            if let Ok(Some(info)) = task.await.unwrap() {
//                urls_info.push(info);
//            }
//        }
//
//        //        urls_info.sort(); // Ensure they are ordered by version
//        for info in urls_info {
//            if info.exists {
//                println!("{:?}", info.url);
//            }
//        }
//    });
//}
