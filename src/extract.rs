use crate::{
    downloads::{DownloadInfo, DownloadList, Extension, Version},
    view::ToHumanSize,
    Config,
};
use anyhow::{anyhow, bail, Context, Result};
use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use indicatif::ProgressBar;
use regex::Regex;
use std::{
    collections::HashSet,
    convert::From,
    fs::File,
    fs::{self},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    result::Result as StdResult,
};
use tar::Archive;

use walkdir::WalkDir;
use xz::read::XzDecoder;

#[derive(Debug)]
pub struct Tarball {
    src: PathBuf,
    ext: Extension,
}

#[derive(Debug, Eq, PartialEq)]
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
    pub fn new(version: Version, extension: Extension) -> Result<Self> {
        let mut src = PathBuf::from(&Config::registry_path()?);
        src.push(format!("php-{version}.tar.{extension}"));

        if !src.exists() {
            bail!("Can't find tarball {src:?}");
        }

        Ok(Self {
            src,
            ext: extension,
        })
    }

    // Download a specific resolved version if we don't have it
    pub async fn get_or_download(version: Version, extension: Extension) -> Result<Self> {
        if Self::new(version, extension).is_err() {
            eprintln!("Unable to find {version} locally, downloading.");
            let downloads = DownloadList::new(version.major, version.minor, extension);
            let dl = downloads
                .get(version)
                .await?
                .context("Unable to get download URL for PHP {version}")?;

            let mut dst = PathBuf::from(&Config::registry_path()?);
            dst.push(version.get_file_name(extension));

            dl.download_to_file(&dst).await?;
        }

        Self::new(version, extension)
    }

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

impl Ord for BuildRoot {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.version.cmp(&other.version)
    }
}

impl PartialOrd for BuildRoot {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl BuildRoot {
    pub fn parent(&self) -> PathBuf {
        let mut parent = self.src.clone();
        parent.pop();
        parent
    }

    pub fn save_manifest(&self) -> Result<(PathBuf, u64)> {
        let mut dst = self.src.clone();
        dst.push(Config::APP_MANIFEST_FILE);

        let mut file = File::create(&dst).context(format!("Failed to open file {dst:?}"))?;

        let mut files = 0u64;

        WalkDir::new(&self.src)
            .into_iter()
            .filter_map(StdResult::ok)
            .filter(|e| !e.path().is_dir())
            .try_for_each(|entry| {
                let suffix = entry
                    .path()
                    .strip_prefix(&self.src)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
                    .to_string_lossy()
                    .into_owned();
                files += 1;
                writeln!(file, "{suffix}")
            })?;

        Ok((dst, files))
    }

    fn load_manifest(&self) -> Result<HashSet<PathBuf>> {
        let mut src = self.src.clone();
        src.push(Config::APP_MANIFEST_FILE);

        let file = File::open(&src).context(format!("Failed to open file {src:?}"))?;
        let reader = BufReader::new(file);
        let set = reader
            .lines()
            .map_while(StdResult::ok)
            .map(PathBuf::from)
            .collect();

        Ok(set)
    }

    pub fn save_scripts<P: AsRef<Path>>(&self, dst_path: P) -> Result<u64> {
        let mut files: u64 = 0;

        let set = self.load_manifest()?;

        fs::create_dir_all(dst_path.as_ref())?;

        let pb = ProgressBar::new_spinner();

        for entry in WalkDir::new(&self.src)
            .into_iter()
            .filter_map(StdResult::ok)
            .filter(|e| !e.path().is_dir())
        {
            let path = entry.path();
            let rel_path = path.strip_prefix(&self.src)?;

            if !set.contains(rel_path) {
                eprintln!("Backing up {path:?}");
                let dst_file_path = dst_path.as_ref().join(rel_path);
                if let Some(parent) = dst_file_path.parent() {
                    fs::create_dir_all(parent)?;
                }

                fs::copy(path, dst_file_path)?;
                files += 1;
                pb.set_message(format!("[{files}] Backing up {rel_path:?}"));
                pb.tick();
            }
        }

        pb.finish_with_message(format!(
            "Backed up {files} files to {}",
            dst_path.as_ref().display()
        ));

        Ok(files)
    }

    pub fn remove(self) -> Result<()> {
        fs::remove_dir_all(self.src)?;
        Ok(())
    }

    fn parse_path_info(dir: &str) -> Result<(Version, &str)> {
        let re = Regex::new(r"php-([0-9]\.[0-9]\.[0-9|a-z|A-Z])\-?(.*)")?;

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

    pub fn new<P: AsRef<Path>>(path: P, version: Version, modifiers: &str) -> Self {
        Self {
            src: path.as_ref().to_path_buf(),
            version,
            modifiers: modifiers.to_string(),
        }
    }

    pub fn from_parent_path<P: AsRef<Path>>(path: P) -> Result<Vec<Self>> {
        let entries = fs::read_dir(&path)
            .with_context(|| format!("Failed to read directory {:?}", &path.as_ref()))?
            .filter_map(StdResult::ok)
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| {
                let path_str = entry.path().to_string_lossy().into_owned();
                match Self::parse_path_info(&path_str) {
                    Ok((version, modifiers)) => Some(Self::new(entry.path(), version, modifiers)),
                    _ => None,
                }
            })
            .collect();

        Ok(entries)
    }

    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let root = path
            .as_ref()
            .file_name()
            .ok_or_else(|| anyhow!("No path component"))?
            .to_str()
            .ok_or_else(|| anyhow!("Filename is not a valid UTF-8 string"))?;

        let (version, modifiers) = Self::parse_path_info(root)?;
        Ok(Self::new(path.as_ref(), version, modifiers))
    }
}
