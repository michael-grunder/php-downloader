use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use futures::future::join_all;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use serde::{de, ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer};
use std::{cmp::Ordering, fmt, io::Write, result::Result as StdResult};

#[derive(Debug)]
pub struct DownloadUrl {
    pub version: Version,
    pub url: String,
    pub size: u64,
    pub date: Option<DateTime<Utc>>,
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
    GZ,
    BZ,
    XY,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord)]
pub enum VersionModifier {
    Alpha,
    Beta,
    RC,
}

#[derive(Debug, Copy, Clone, Eq)]
pub struct Version {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
    pub had_patch: bool,
    pub rc: Option<VersionModifier>,
}

impl std::default::Default for Extension {
    fn default() -> Self {
        Self::BZ
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

        Ok(Self::new(major, minor, patch, parts.len() == 3, None))
    }
}

impl DownloadUrl {
    pub fn new(version: Version, url: &str, size: u64, date: Option<DateTime<Utc>>) -> Self {
        Self {
            version,
            url: url.to_string(),
            size,
            date,
        }
    }

    pub fn date_string(&self) -> String {
        self.date
            .map_or_else(String::new, |d| d.format("%d %b %y").to_string())
    }

    pub async fn download(&self, file: &mut std::fs::File) -> Result<()> {
        let mut response = reqwest::get(&self.url).await?;

        let total_size = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|val| val.to_str().ok())
            .and_then(|val| val.parse::<u64>().ok())
            .unwrap_or(0);

        let tmpl = "{msg} {spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})";

        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(tmpl)?
                .progress_chars("#>-"),
        );
        pb.set_message(self.version.to_string());

        while let Some(chunk) = response.chunk().await? {
            pb.inc(chunk.len() as u64);
            file.write_all(&chunk)?;
        }

        pb.finish_with_message("download completed");
        Ok(())
    }
}

impl Version {
    pub const fn from_major_minor(major: u8, minor: u8) -> Self {
        Self::new(major, minor, 0, false, None)
    }

    pub const fn from_major_minor_patch(major: u8, minor: u8, patch: u8) -> Self {
        Self::new(major, minor, patch, true, None)
    }

    pub const fn new(
        major: u8,
        minor: u8,
        patch: u8,
        had_patch: bool,
        rc: Option<VersionModifier>,
    ) -> Self {
        Self {
            major,
            minor,
            patch,
            had_patch,
            rc,
        }
    }

    pub fn get_file_name(self, extension: Extension) -> String {
        format!(
            "php-{}.{}.{}.tar.{}",
            self.major, self.minor, self.patch, extension
        )
    }

    fn get_url(self, extension: Extension) -> String {
        if self.major <= 7 && self.minor < 4 {
            format!(
                "https://museum.php.net/php{}/php-{self}.tar.{extension}",
                self.major
            )
        } else if self.major == 8 && self.minor > 2 {
            format!("https://downloads.php.net/~jakub/php-{self}.tar.{extension}",)
        } else {
            format!("https://php.net/distributions/php-{self}.tar.{extension}")
        }
    }

    pub async fn resolve_latest(&mut self, dl: &DownloadList) -> Result<()> {
        if !self.had_patch {
            *self = dl
                .latest()
                .await?
                .ok_or_else(|| anyhow!("Failed to resolve the latest patch for version {}", self))?
                .version;
        }

        Ok(())
    }
}

impl Serialize for DownloadUrl {
    fn serialize<S>(&self, serializer: S) -> StdResult<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DownloadUrl", 4)?;

        state.serialize_field("version", &self.version)?;
        state.serialize_field("url", &self.url)?;
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
        std::str::FromStr::from_str(&s).map_err(de::Error::custom)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.rc {
            Some(rc) => write!(f, "{}.{}.0{}{}", self.major, self.minor, rc, self.patch),
            None => write!(f, "{}.{}.{}", self.major, self.minor, self.patch),
        }
    }
}

impl std::fmt::Display for VersionModifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v = match self {
            Self::Alpha => "alpha",
            Self::Beta => "beta",
            Self::RC => "RC",
        };

        write!(f, "{v}")
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
        match (
            self.major.cmp(&other.major),
            self.minor.cmp(&other.minor),
            self.rc.cmp(&other.rc),
            self.patch.cmp(&other.patch),
        ) {
            (Ordering::Equal, Ordering::Equal, Ordering::Equal, rc) => rc,
            (Ordering::Equal, Ordering::Equal, patch, _) => patch,
            (Ordering::Equal, minor, _, _) => minor,
            (major, _, _, _) => major,
        }
    }
}

impl VersionModifier {
    pub fn variants() -> Vec<Self> {
        vec![Self::Alpha, Self::Beta, Self::RC]
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

    async fn get_header(&self, version: Version) -> Result<Option<DownloadUrl>> {
        let url = version.get_url(self.extension);
        let res = self.client.head(&url).send().await?;

        if res.status().is_success() {
            let headers = res.headers();

            let content_length = headers
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|str_val| str_val.parse::<u64>().ok())
                .unwrap_or(0);

            let last_modified = headers
                .get(reqwest::header::LAST_MODIFIED)
                .and_then(|value| value.to_str().ok())
                .and_then(|str_val| DateTime::parse_from_rfc2822(str_val).ok())
                .map(|datetime| datetime.with_timezone(&Utc));

            Ok(Some(DownloadUrl::new(
                version,
                &url,
                content_length,
                last_modified,
            )))
        } else {
            Ok(None)
        }
    }

    fn get_check_versions(&self) -> Box<dyn Iterator<Item = Version> + '_> {
        if self.major == 8 && self.minor == 3 {
            Box::new(VersionModifier::variants().into_iter().flat_map(move |m| {
                (1..8).map(move |n| Version::new(self.major, self.minor, n, true, Some(m)))
            }))
        } else {
            Box::new(
                (0..30).map(|patch| Version::from_major_minor_patch(self.major, self.minor, patch)),
            )
        }
    }

    pub async fn list(&self) -> Result<Vec<DownloadUrl>> {
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

    pub async fn latest(&self) -> Result<Option<DownloadUrl>> {
        let mut urls = self.list().await?;
        Ok(urls.pop())
    }

    pub async fn get(&self, version: Version) -> Result<Option<DownloadUrl>> {
        self.get_header(version).await
    }
}
