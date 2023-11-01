use crate::{
    downloads::{DownloadInfo, Extension, Version},
    view::ToHumanSize,
};
use anyhow::{anyhow, Result};
use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use indicatif::ProgressBar;
use std::{
    convert::From,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    result::Result as StdResult,
};
use tar::Archive;
use xz::read::XzDecoder;

#[derive(Debug)]
pub struct Tarball {
    src: PathBuf,
    ext: Extension,
}

struct ProgressReader<R> {
    reader: R,
    progress_bar: ProgressBar,
}

pub trait Extract {
    fn extract(&self, dst_root: &Path, dst_leaf: Option<&Path>) -> Result<()>;
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

impl Extract for Tarball {
    fn extract(&self, dst_root: &Path, dst_leaf: Option<&Path>) -> Result<()> {
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

        Ok(())
    }
}

impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let bytes = self.reader.read(buf)?;
        self.progress_bar.tick();
        Ok(bytes)
    }
}
