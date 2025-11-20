use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use futures::future::join_all;
use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use reqwest::Client;
use serde::{
    de, ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer,
};
use std::{
    fmt, fs, io::Write, os::unix::fs::PermissionsExt, path::Path,
    result::Result as StdResult, str::FromStr,
};
use tempfile::NamedTempFile;

#[derive(Debug)]
pub struct DownloadInfo {
    pub location: String,
    pub version: Version,
    pub size: u64,
    pub date: Option<DateTime<Utc>>,
    pub extension: Extension,
}

#[derive(Debug)]
pub struct DownloadList {
    client: Client,
    major: u8,
    minor: u8,
    extension: Extension,
}

#[derive(Debug, Clone, Copy)]
pub enum Extension {
    BZ,
    GZ,
    XZ,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord)]
pub enum VersionModifier {
    Alpha,
    Beta,
    RC(u8),
}

#[derive(Debug, Copy, Clone, Eq)]
pub struct Version {
    pub major: u8,
    pub minor: u8,
    pub patch: Option<u8>,
    pub rc: Option<VersionModifier>,
}

impl FromStr for Extension {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match &*s.to_lowercase() {
            "bz2" | "bz" => Ok(Self::BZ),
            "gz" => Ok(Self::GZ),
            "xz" => Ok(Self::XZ),
            _ => Err(anyhow!("Unknown extension")),
        }
    }
}

impl fmt::Display for Extension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ext = match self {
            Self::BZ => "bz2",
            Self::GZ => "gz",
            Self::XZ => "xy",
        };

        write!(f, "{ext}")
    }
}

impl FromStr for Version {
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

        let (modifier, patch) = if parts.len() == 3 {
            VersionModifier::from_patch(parts[2])?
        } else {
            (None, None)
        };

        Ok(Self::new(major, minor, patch, modifier))
    }
}

impl FromStr for VersionModifier {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let re = Regex::new(r"(?i)(alpha|beta|rc)(\d*)?")
            .expect("Can't parse regex");

        match re.captures(s) {
            Some(caps) => {
                match &*caps.get(1).unwrap().as_str().to_lowercase() {
                    "alpha" => Ok(Self::Alpha),
                    "beta" => Ok(Self::Beta),
                    "rc" => match caps.get(2) {
                        Some(n) => {
                            let num = n.as_str().parse::<u8>()?;
                            Ok(Self::RC(num))
                        }
                        None => Err(anyhow!(
                            "Failed to parse version modifier {s:?}"
                        )),
                    },
                    _ => unreachable!(),
                }
            }
            None => Err(anyhow!("Don't understand version modifier {s:?}")),
        }
    }
}

impl VersionModifier {
    fn from_patch(s: &str) -> Result<(Option<Self>, Option<u8>)> {
        let re = Regex::new(r"^(\d+)(.*)$").expect("Can't parse regex");

        match re.captures(s) {
            Some(caps) => {
                let patch_u8 = caps
                    .get(1)
                    .ok_or_else(|| {
                        anyhow!("No match for the first capture group")
                    })?
                    .as_str()
                    .parse::<u8>()?;

                let modifier = caps
                    .get(2)
                    .ok_or_else(|| {
                        anyhow!("Failed to parse second capture group")
                    })?
                    .as_str();

                if modifier.is_empty() {
                    Ok((None, Some(patch_u8)))
                } else {
                    let modifier = Self::from_str(modifier)?;
                    Ok((Some(modifier), Some(patch_u8)))
                }
            }
            None => Err(anyhow!(format!("Unable to parse patch {s:?}"))),
        }
    }

    pub fn to_u32(&self) -> u32 {
        match self {
            Self::Alpha => 10,
            Self::Beta => 9,
            Self::RC(n) => 9 - u32::from(*n),
        }
    }
}

impl DownloadInfo {
    pub fn new(
        version: Version,
        location: &str,
        size: u64,
        date: Option<DateTime<Utc>>,
        extension: Extension,
    ) -> Self {
        Self {
            version,
            location: location.to_string(),
            size,
            date,
            extension,
        }
    }

    pub fn date_string(&self) -> String {
        self.date
            .map_or_else(String::new, |d| d.format("%d %b %y").to_string())
    }

    fn clean_file_name(file: &Path) -> String {
        let mut file: String = file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into();

        file = file.replace("php-", "");
        for ext in Extension::variants() {
            file = file.replace(&format!(".tar.{ext}"), "");
        }

        file
    }

    /// Take a path and convert it into a `DownloadInfo` struct.
    ///
    /// # Errors
    /// Can fail if we can't parse the version or extension from the file name.
    pub fn from_file(file: &Path) -> Result<Self> {
        let ext = file.extension().unwrap_or_default().to_string_lossy();

        Ok(Self::new(
            Self::clean_file_name(file).parse()?,
            &file.to_string_lossy(),
            std::fs::metadata(file).map(|m| m.len()).unwrap_or(0),
            None,
            ext.parse()?,
        ))
    }

    /// Attempt to download a PHP version to a specific destination file.
    ///
    /// # Errors
    ///
    /// This will fail if we can't create the file or execute the download.
    pub async fn download_to_file(&self, dst: &Path) -> Result<()> {
        let parent = dst.parent().ok_or_else(|| {
            anyhow!(
                "Destination path {} has no parent directory",
                dst.display()
            )
        })?;

        // Important: create the temp file *in the same directory* as dst so
        // that the final rename does not cross filesystems.
        let mut tmp = NamedTempFile::new_in(parent).with_context(|| {
            format!(
                "Unable to create temporary file in directory {}",
                parent.display()
            )
        })?;

        let mut perms = fs::metadata(tmp.path())?.permissions();
        perms.set_mode(0o644);
        fs::set_permissions(tmp.path(), perms)?;

        // If download fails, the temp file will be dropped and removed.
        self.download(tmp.as_file_mut()).await.with_context(|| {
            format!(
                "Failed to download {} into temporary file {}",
                self.version,
                tmp.path().display(),
            )
        })?;

        // Persist: this is a rename(2) under the hood.
        tmp.persist(dst).map_err(|err| {
            let src = err.file.path().to_path_buf();
            let io_err = err.error;

            anyhow!(
                "Failed to persist temporary file.\n  from: {}\n  to:   {}\n  \
                 cause: {io_err}",
                src.display(),
                dst.display(),
            )
        })?;

        Ok(())
    }

    /// Download data to a generic writer.
    ///
    /// # Errors
    /// This can fail if the download fails or we can't write to the writer.
    pub async fn download<W>(&self, writer: &mut W) -> Result<()>
    where
        W: Write + Send,
    {
        let mut response = reqwest::get(&self.location).await?;

        let total_size = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|val| val.to_str().ok())
            .and_then(|val| val.parse::<u64>().ok())
            .unwrap_or(0);

        let tmpl = concat!(
            "{msg} {spinner:.green} [{elapsed_precise}] ",
            "[{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
        );

        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(tmpl)?
                .progress_chars("#>-"),
        );
        pb.set_message(self.version.to_string());

        while let Some(chunk) = response.chunk().await? {
            pb.inc(chunk.len() as u64);
            writer.write_all(&chunk)?;
        }

        pb.finish_with_message("download completed");
        Ok(())
    }
}

//function rc_value($rc) {
//    switch ($rc) {
//        case '':
//            return 0;
//        case 'alpha':
//            return -1;
//        case 'beta':
//            return -2;
//        default:
//            return -2 - rc_num($rc);
//    }
//}
//
//function to_number($major, $minor, $patch, $rc) {
//    $maj_part = $major * 1000000;
//    $min_part = $minor * 10000;
//    $ptc_part = $patch * 100;
//    $rc_part  = rc_value($rc);

//impl From<Version> for u32 {
//    fn from(v: Version) -> Self {
//        Self::from(v.major) * 1_000_000
//            + Self::from(v.minor) * 10_000
//            + Self::from(v.patch.unwrap_or(0)) * 100
//    }
//}

impl From<VersionModifier> for i32 {
    fn from(m: VersionModifier) -> Self {
        match m {
            VersionModifier::Alpha => -10,
            VersionModifier::Beta => -9,
            VersionModifier::RC(n) => -9 + Self::from(n),
        }
    }
}

impl Version {
    pub const fn from_major_minor(major: u8, minor: u8) -> Self {
        Self::new(major, minor, None, None)
    }

    pub const fn from_major_minor_patch(
        major: u8,
        minor: u8,
        patch: u8,
    ) -> Self {
        Self::new(major, minor, Some(patch), None)
    }

    pub const fn new(
        major: u8,
        minor: u8,
        patch: Option<u8>,
        rc: Option<VersionModifier>,
    ) -> Self {
        Self {
            major,
            minor,
            patch,
            rc,
        }
    }

    pub fn get_file_name(self, extension: Extension) -> String {
        format!("php-{self}.tar.{extension}")
    }

    fn get_url(self, extension: Extension) -> String {
        if self.major <= 7 && self.minor < 4 {
            format!(
                "https://museum.php.net/php{}/php-{self}.tar.{extension}",
                self.major
            )
        } else {
            format!("https://php.net/distributions/php-{self}.tar.{extension}")
        }
    }

    /// Given potentially partial version information attempt to figure out what the actual latest
    /// version available for download is.
    ///
    /// For example if the user wants 7.4 we go looking for which 7.4.N is the newest.
    ///
    /// # Errors
    ///
    /// This can fail if we can't  retrieve the info from the remote host.
    pub async fn resolve_latest(&mut self, dl: &DownloadList) -> Result<()> {
        if self.patch.is_none() {
            *self = dl
                .latest()
                .await?
                .ok_or_else(|| {
                    anyhow!(
                        "Failed to resolve the latest patch for version {self}"
                    )
                })?
                .version;
        }

        Ok(())
    }

    pub const fn matches(self, other: Self) -> bool {
        if self.major != other.major || self.minor != other.minor {
            return false;
        }

        match (self.patch, other.patch) {
            (Some(a), Some(b)) => a == b,
            (Some(_) | None, None | Some(_)) => true,
        }
    }

    pub const fn optional_matches(self, other: Option<Self>) -> bool {
        match other {
            Some(o) => self.matches(o),
            _ => true,
        }
    }

    pub fn to_u32(&self) -> u32 {
        u32::from(self.major) * 1_000_000
            + u32::from(self.minor) * 10_000
            + u32::from(self.patch.unwrap_or(0)) * 100
            - self.rc.map_or(0, |m| m.to_u32())
    }
}

impl Serialize for DownloadInfo {
    fn serialize<S>(&self, serializer: S) -> StdResult<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DownloadInfo", 4)?;

        state.serialize_field("version", &self.version)?;
        state.serialize_field("location", &self.location)?;
        state.serialize_field("size", &self.size)?;

        // Serializing date as a String in the "YYYY/MM/DD" format
        if let Some(date) = &self.date {
            let date_str = date.format("%Y/%m/%d").to_string();
            state.serialize_field("date", &date_str)?;
        } else {
            state.serialize_field("date", &None::<String>)?;
        }

        state.end()
    }
}

impl Serialize for Version {
    fn serialize<S>(&self, serializer: S) -> StdResult<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        FromStr::from_str(&s).map_err(de::Error::custom)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.rc {
            Some(rc) => write!(
                f,
                "{}.{}.{}{}",
                self.major,
                self.minor,
                self.patch.unwrap_or(0),
                rc,
            ),
            None => {
                if let Some(patch) = self.patch {
                    write!(f, "{}.{}.{}", self.major, self.minor, patch,)
                } else {
                    write!(f, "{}.{}", self.major, self.minor,)
                }
            }
        }
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.major == other.major
            && self.minor == other.minor
            && self.patch == other.patch
            && self.rc == other.rc
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.to_u32().cmp(&other.to_u32())
    }
}

impl fmt::Display for VersionModifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = match self {
            Self::Alpha => "alpha".into(),
            Self::Beta => "beta".into(),
            Self::RC(n) => format!("RC{n}"),
        };

        write!(f, "{v}")
    }
}

impl Extension {
    pub fn variants() -> Vec<Self> {
        vec![Self::GZ, Self::BZ, Self::XZ]
    }
}

impl DownloadList {
    pub fn new(major: u8, minor: u8, extension: Extension) -> Self {
        Self {
            client: Client::new(),
            major,
            minor,
            extension,
        }
    }

    async fn get_header(
        &self,
        version: Version,
    ) -> Result<Option<DownloadInfo>> {
        let url = version.get_url(self.extension);
        let res = self.client.head(&url).send().await?;

        if res.status().is_success() {
            let content_length = res
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|str_val| str_val.parse::<u64>().ok())
                .unwrap_or(0);

            let last_modified = res
                .headers()
                .get(reqwest::header::LAST_MODIFIED)
                .and_then(|value| value.to_str().ok())
                .and_then(|str_val| DateTime::parse_from_rfc2822(str_val).ok())
                .map(|datetime| datetime.with_timezone(&Utc));

            Ok(Some(DownloadInfo::new(
                version,
                &url,
                content_length,
                last_modified,
                self.extension,
            )))
        } else {
            Ok(None)
        }
    }

    fn get_check_versions(&self) -> Box<dyn Iterator<Item = Version> + '_> {
        Box::new((0..31).map(|patch| {
            Version::from_major_minor_patch(self.major, self.minor, patch)
        }))
    }

    /// List versions available for download.
    ///
    /// # Errors
    ///
    /// This can fail if we have troulbe reading data from the remote host.
    pub async fn list(&self) -> Result<Vec<DownloadInfo>> {
        let urls: Vec<_> = self
            .get_check_versions()
            .map(|version| self.get_header(version))
            .collect();

        let mut urls: Vec<_> = join_all(urls)
            .await
            .into_iter()
            .filter_map(Result::ok)
            .flatten()
            .collect();

        urls.sort_unstable_by(|b, a| b.version.cmp(&a.version));

        Ok(urls)
    }

    /// Pop the latest version from our list
    ///
    /// # Errors
    ///
    /// This can fail if our list is empty
    pub async fn latest(&self) -> Result<Option<DownloadInfo>> {
        let mut urls = self.list().await?;
        Ok(urls.pop())
    }

    /// Get download information for a specific version.
    ///
    /// # Errors
    ///
    /// This can fail if we can't read the header.
    pub async fn get(&self, version: Version) -> Result<Option<DownloadInfo>> {
        self.get_header(version).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parsing() {
        let versions = &[
            ("7.4.0", Version::new(7, 4, Some(0), None)),
            ("7.4.1", Version::new(7, 4, Some(1), None)),
            (
                "8.0.0alpha",
                Version::new(8, 0, Some(0), Some(VersionModifier::Alpha)),
            ),
            (
                "8.0.0beta",
                Version::new(8, 0, Some(0), Some(VersionModifier::Beta)),
            ),
            (
                "8.0.0RC1",
                Version::new(8, 0, Some(0), Some(VersionModifier::RC(1))),
            ),
            (
                "8.0.0RC2",
                Version::new(8, 0, Some(0), Some(VersionModifier::RC(2))),
            ),
        ];

        for (s, expected) in versions {
            assert_eq!(
                Version::from_str(s).expect("Can't parse version"),
                *expected,
                "Failed to parse version {s:?}",
            );
        }
    }

    #[test]
    fn test_version_sorting() {
        let versions = &[
            "7.4.1",
            "7.4.0",
            "8.3.0beta",
            "8.3.0",
            "8.3.0RC2",
            "8.3.0alpha",
            "8.3.0RC1",
        ];

        let sorted = &[
            "7.4.0",
            "7.4.1",
            "8.3.0alpha",
            "8.3.0beta",
            "8.3.0RC1",
            "8.3.0RC2",
            "8.3.0",
        ];

        let mut mapped: Vec<_> = versions
            .iter()
            .map(|s| (s, Version::from_str(s).expect("Can't parse")))
            .collect(); // Collect into a Vec<(str, Version)>

        // Sort the vector by the Version part
        mapped.sort_by(|a, b| a.1.cmp(&b.1));

        // Extract the string parts from the sorted tuples
        let sorted_strings: Vec<&str> =
            mapped.iter().map(|(s, _)| **s).collect();

        // Compare the sorted strings with the expected sorted array
        assert_eq!(sorted_strings, sorted);
    }

    #[test]
    fn parse_rc_version() {
        let version_str = "8.3.0RC5";
        let version =
            Version::from_str(version_str).expect("Can't parse version string");
        assert_eq!(
            version,
            Version::new(8, 3, Some(0), Some(VersionModifier::RC(5)))
        );

        assert_eq!("8.3.0RC5", version.to_string());
    }
}
