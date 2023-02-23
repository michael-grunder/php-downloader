// Clippy:
#![warn(clippy::all, clippy::nursery, clippy::pedantic)]
#![allow(clippy::non_ascii_literal)]

use anyhow::anyhow;
use clap::Parser;
use downloader::Downloader;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use serde::{de, Deserialize, Deserializer};

use std::{fmt::Write, path::PathBuf};

//static PHP_KEYS: &[u8] = include_bytes!("../php-keyring.gpg");

#[derive(Debug, Clone, Default)]
struct Version {
    major: u16,
    minor: u16,
    patch: u16,
}

#[derive(Debug, Clone)]
enum Extension {
    GZ,
    BZ,
    XY,
}

#[derive(Parser, Debug)]
struct Options {
    #[arg(short, long)]
    verify: bool,

    #[arg(short, long, default_value = "bz2")]
    extension: Extension,

    version: Version,

    #[arg(default_value = ".")]
    output_path: PathBuf,
}

impl Extension {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::GZ => "gz",
            Self::BZ => "bz2",
            Self::XY => "xy",
        }
    }
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

impl std::str::FromStr for Version {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('.').collect();

        if parts.len() != 3 {
            return Err(anyhow!("Invalid version string '{s}'"));
        }

        let major = parts[0]
            .parse()
            .map_err(|_| anyhow!("Invalid major version"))?;
        let minor = parts[1]
            .parse()
            .map_err(|_| anyhow!("Invalid minor version"))?;
        let patch = parts[2]
            .parse()
            .map_err(|_| anyhow!("Invalid patch version"))?;

        Ok(Self::new(major, minor, patch))
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

    fn get_file_name(&self) -> PathBuf {
        PathBuf::from(format!(
            "php-{}.{}.{}.tar.bz2",
            self.major, self.minor, self.patch
        ))
    }

    fn get_temp_file() -> tempfile::NamedTempFile {
        tempfile::NamedTempFile::new().expect("Can't create temporary file")
    }

    fn get_urls(&self) -> Vec<(tempfile::NamedTempFile, String)> {
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

// Define a custom progress reporter:
struct SimpleReporterPrivate {
    max_progress: Option<u64>,
    progress_bar: ProgressBar,
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

impl downloader::progress::Reporter for SimpleReporter {
    fn setup(&self, max_progress: Option<u64>, _: &str) {
        let max = max_progress.unwrap_or(0);
        let progress_bar = ProgressBar::new(max);
        progress_bar.set_style(ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .with_key("eta", |state: &ProgressState, w: &mut dyn Write| write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap())
            .progress_chars("#>-"));

        let private = SimpleReporterPrivate {
            max_progress,
            progress_bar,
        };

        let mut guard = self.private.lock().unwrap();
        *guard = Some(private);
    }

    fn progress(&self, current: u64) {
        if let Some(p) = self.private.lock().unwrap().as_mut() {
            if p.max_progress.is_some() {
                p.progress_bar.set_position(current);
            }
        }
    }

    fn set_message(&self, _: &str) {}

    fn done(&self) {
        if let Some(p) = self.private.lock().unwrap().as_mut() {
            if let Some(max) = p.max_progress {
                p.progress_bar.set_position(max);
            }
        }

        let mut guard = self.private.lock().unwrap();
        *guard = None;
    }
}

fn main() -> anyhow::Result<()> {
    let opt: Options = Options::parse();

    let mut downloader = Downloader::builder()
        //        .download_folder(std::path::Path::new("/tmp"))
        .parallel_requests(2)
        .build()
        .unwrap();

    let mut downloads: Vec<downloader::Download> = vec![];

    for (file, url) in opt.version.get_urls() {
        let mut dl = downloader::Download::new(&url);
        dl = dl.file_name(file.path());
        dl = dl.progress(SimpleReporter::create());
        downloads.push(dl);
    }

    let result = downloader.download(&downloads)?;

    for summary in result.into_iter().flatten() {
        let mut path = opt.output_path;
        path.set_file_name(opt.version.get_file_name());

        std::fs::rename(summary.file_name, path)?;
        std::process::exit(0);
    }

    std::process::exit(1);
}
