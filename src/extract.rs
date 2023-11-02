use crate::{
    downloads::{DownloadInfo, Extension, Version},
    view::ToHumanSize,
    Config,
};
use anyhow::{anyhow, bail, Result};
use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use indicatif::ProgressBar;
use regex::Regex;
use std::{
    convert::From,
    fs::File,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    result::Result as StdResult,
    time::{SystemTime, UNIX_EPOCH},
};
use tar::Archive;

use walkdir::WalkDir;
use xz::read::XzDecoder;

#[derive(Debug)]
pub struct Tarball {
    src: PathBuf,
    ext: Extension,
}

#[derive(Debug)]
pub struct BuildRoot {
    pub src: PathBuf,
    pub version: Version,
    pub modifiers: String,
}

struct ProgressReader<R> {
    reader: R,
    progress_bar: ProgressBar,
}

struct ProgressWriter<W> {
    name: String,
    writer: W,
    progress_bar: ProgressBar,
    bytes_written: usize,
}

impl Tarball {
    fn progress_spinner(&self, size: u64, dst: &Path) -> Result<ProgressBar> {
        let file = self
            .src
            .file_name()
            .ok_or_else(|| anyhow!("Can't get filename"))?
            .to_string_lossy();

        let pb = ProgressBar::new_spinner();

        pb.set_message(format!(
            "{file} ({}) -> {}",
            size.to_human_size(),
            dst.display(),
        ));

        Ok(pb)
    }

    fn clean_file_name(&self) -> Result<PathBuf> {
        let file = self
            .src
            .file_name()
            .ok_or_else(|| anyhow!("Can't get filename"))?
            .to_string_lossy()
            .into_owned();

        if let Some((version, _)) = file.split_once(".tar") {
            Ok(PathBuf::from(version))
        } else {
            anyhow::bail!("Unable to determine stem for '{}'", file);
        }
    }

    fn full_path(root: &Path, leaf: &Path) -> PathBuf {
        let mut full: PathBuf = root.to_path_buf();
        full.push(leaf);
        full
    }

    pub fn list(dir: &Path) -> Result<Vec<DownloadInfo>> {
        let res: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(StdResult::ok)
            .filter(|p| !p.path().is_dir())
            .filter_map(|path| {
                DownloadInfo::from_file(&path.path()).map_or_else(
                    |_| {
                        eprintln!("Can't parse file '{path:?}'");
                        None
                    },
                    Some,
                )
            })
            .collect();

        Ok(res)
    }

    pub fn matching(path: &Path, version: Version) -> Result<Vec<DownloadInfo>> {
        Ok(Self::list(path)?
            .into_iter()
            .filter(|fi| fi.version.matches(version))
            .collect())
    }

    pub fn latest(path: &Path, version: Version) -> Result<Option<DownloadInfo>> {
        Ok(Self::matching(path, version)?.pop())
    }

    pub fn validate_writable_directory(path: &Path) -> Result<()> {
        if !path.exists() {
            bail!("Path '{}' does not exist", path.display());
        } else if !path.is_dir() {
            bail!("Path '{}' is not a directory", path.display());
        } else if fs::metadata(path)?.permissions().readonly() {
            bail!("Path '{}' is not writable", path.display());
        }

        Ok(())
    }

    pub fn extract(&self, dst_root: &Path, dst_leaf: Option<&Path>) -> Result<PathBuf> {
        let file = File::open(&self.src)?;
        let total_size = file.metadata()?.len();

        let decoder: Box<dyn Read> = match self.ext {
            Extension::GZ => Box::new(GzDecoder::new(file)),
            Extension::BZ => Box::new(BzDecoder::new(file)),
            Extension::XZ => Box::new(XzDecoder::new(file)),
        };

        let tmp = tempfile::tempdir()?;
        let def = self.clean_file_name()?;
        let src = Self::full_path(tmp.path(), &def);
        let dst = Self::full_path(dst_root, dst_leaf.unwrap_or(&def));

        let reader = ProgressReader {
            reader: decoder,
            progress_bar: self.progress_spinner(total_size, &dst)?,
        };

        let mut archive = Archive::new(reader);
        archive.unpack(&tmp)?;

        std::fs::rename(src, &dst)?;
        eprintln!("Files extracted to '{}'", dst.display());

        Ok(dst)
    }

    pub fn touch_sentinel<P: AsRef<Path>>(path: P) -> Result<()> {
        let mut full_path = PathBuf::from(path.as_ref());
        full_path.push(Config::app_sentinel_file());

        Self::touch(full_path)
    }

    pub fn touch<P: AsRef<Path>>(path: P) -> Result<()> {
        let _ = OpenOptions::new()
            .create(true)
            .write(true)
            .open(path.as_ref())?;

        let now = SystemTime::now();
        filetime::set_file_times(path, now.into(), now.into())?;

        Ok(())
    }
}

impl From<&DownloadInfo> for Tarball {
    fn from(src: &DownloadInfo) -> Self {
        Self {
            src: PathBuf::from(&src.location),
            ext: src.extension,
        }
    }
}

impl From<DownloadInfo> for Tarball {
    fn from(src: DownloadInfo) -> Self {
        Self {
            src: PathBuf::from(src.location),
            ext: src.extension,
        }
    }
}

impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let bytes = self.reader.read(buf)?;
        self.progress_bar.tick();
        Ok(bytes)
    }
}

impl<W: Write> Write for ProgressWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let bytes = self.writer.write(buf)?;
        self.bytes_written += bytes;
        self.progress_bar.set_message(format!(
            "Writing {} ({})",
            self.name,
            (self.bytes_written as u64).to_human_size()
        ));

        Ok(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl BuildRoot {
    fn file_timestamp<P: AsRef<Path>>(file: P) -> Result<u64> {
        let timestamp = fs::metadata(file)?
            .created()?
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Ok(timestamp)
    }

    fn sentinel_timestamp(&self) -> Result<u64> {
        let mut path = self.src.clone();
        path.push(Config::app_sentinel_file());
        Self::file_timestamp(path)
    }

    fn is_script_ext<P: AsRef<Path>>(path: P) -> bool {
        let ext = path
            .as_ref()
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");

        matches!(ext, "php" | "py" | "sh" | "c" | "cc" | "conf" | "log")
    }

    pub fn save_scripts<P: AsRef<Path>>(&self, dst_path: P) -> Result<()> {
        let sentinel = self.sentinel_timestamp()?;

        fs::create_dir_all(dst_path.as_ref())?;

        for entry in WalkDir::new(&self.src)
            .into_iter()
            .filter_map(StdResult::ok)
            .filter(|e| !e.path().is_dir())
        {
            let path = entry.path();
            let created = Self::file_timestamp(path)?;

            if created > sentinel && Self::is_script_ext(path) {
                eprintln!("Backing up {path:?}");
                let rel_path = path.strip_prefix(&self.src)?;
                let dst_file_path = dst_path.as_ref().join(rel_path);
                if let Some(parent) = dst_file_path.parent() {
                    fs::create_dir_all(parent)?;
                }

                fs::copy(path, dst_file_path)?;
            }
        }

        Ok(())
    }

    fn user_confirm(msg: &str) -> Result<bool> {
        print!("{msg}? (yes/no)");

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        let s = input.trim().to_lowercase();
        Ok(matches!(s.as_str(), "yes" | "y"))
    }

    pub fn remove(self, verify: bool) -> Result<()> {
        if !verify || Self::user_confirm(&format!("Delete old tree {:?}", self.src))? {
            fs::remove_dir_all(self.src)?;
        }
        Ok(())
    }

    fn parse_path_info(dir: &str) -> Result<(Version, &str)> {
        let re = Regex::new(r"php-([0-9]\.[0-9]\.[0-9|a-z|A-Z])(.*)")?;

        if let Some(caps) = re.captures(dir) {
            let version = caps
                .get(1)
                .ok_or_else(|| anyhow!("Failed extract version from path."))?
                .as_str()
                .parse()?;

            let modifiers = caps.get(2).map_or("", |m| m.as_str());

            Ok((version, modifiers))
        } else {
            bail!("Failed to parse '{dir}' for version information");
        }
    }

    pub fn version_path_name(&self, version: Version) -> String {
        if self.modifiers.is_empty() {
            format!("php-{version}")
        } else {
            format!("php-{version}-{}", self.modifiers)
        }
    }

    pub fn new(path: &Path, version: Version, modifiers: &str) -> Self {
        Self {
            src: path.to_path_buf(),
            version,
            modifiers: modifiers.to_string(),
        }
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let root = path
            .file_name()
            .ok_or_else(|| anyhow!("No path component"))?
            .to_str()
            .ok_or_else(|| anyhow!("Filename is not a valid UTF-8 string"))?;

        let (version, modifiers) = Self::parse_path_info(root)?;

        Ok(Self::new(path, version, modifiers))
    }
}
