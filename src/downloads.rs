use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use futures::future::join_all;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use serde::{de, Deserialize, Deserializer};
use std::{fmt, io::Write};

#[derive(Debug)]
pub struct DownloadUrl {
    pub version: Version,
    pub url: String,
    pub size: u64,
    pub date: Option<DateTime<Utc>>,
    pub age: String,
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

#[derive(Debug, Copy, Clone, Eq)]
pub struct Version {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
    pub had_patch: bool,
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

        Ok(Self::new_ex(major, minor, patch, parts.len() == 3))
    }
}

impl DownloadUrl {
    pub fn new(version: Version, url: &str, size: u64, date: Option<DateTime<Utc>>) -> Self {
        Self {
            version,
            url: url.to_string(),
            size,
            date,
            age: date.map_or_else(String::new, |d| Self::get_age(&d)),
        }
    }

    fn get_age(date: &DateTime<Utc>) -> String {
        let now = Utc::now();
        let duration = now.signed_duration_since(date);

        let years = duration.num_days() / 365;
        let remaining_days_year = duration.num_days() % 365;

        let months = remaining_days_year / 30;
        let remaining_days_month = remaining_days_year % 30;

        let days = remaining_days_month;

        let hours = duration.num_hours() % 24;
        let minutes = duration.num_minutes() % 60;

        let parts = if years > 0 {
            vec![(years, "year"), (months, "month")]
        } else if months > 0 {
            vec![(months, "month"), (days, "day")]
        } else if days > 0 {
            vec![(days, "day"), (hours, "hour")]
        } else {
            vec![(hours, "hour"), (minutes, "minute")]
        };

        parts
            .into_iter()
            .filter(|(v, _)| v > &0)
            .map(|(v, ident)| format!("{v} {ident}{}", if v > 1 { "s" } else { "" }))
            .collect::<Vec<String>>()
            .join(", ")
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
    pub const fn new(major: u8, minor: u8, patch: u8) -> Self {
        Self {
            major,
            minor,
            patch,
            had_patch: true,
        }
    }

    pub const fn new_ex(major: u8, minor: u8, patch: u8, had_patch: bool) -> Self {
        Self {
            major,
            minor,
            patch,
            had_patch,
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
                "https://museum.php.net/php{}/php-{}.{}.{}.tar.{}",
                self.major, self.major, self.minor, self.patch, extension
            )
        } else {
            format!(
                "https://php.net/distributions/php-{}.{}.{}.tar.{}",
                self.major, self.minor, self.patch, extension
            )
        }
    }

    pub async fn resolve_patch(mut self, dl: &DownloadList) -> Result<()> {
        if !self.had_patch {
            self.patch = dl
                .latest()
                .await?
                .ok_or_else(|| anyhow!("Failed to resolve the latest patch for version {}", self))?
                .version
                .patch;
        }

        Ok(())
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
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.major == other.major && self.minor == other.minor && self.patch == other.patch
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
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

    pub async fn list(&self) -> Result<Vec<DownloadUrl>> {
        let urls: Vec<_> = (0..30)
            .map(|patch| Version::new(self.major, self.minor, patch))
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

    pub async fn get(&self, patch: u8) -> Result<Option<DownloadUrl>> {
        let version = Version::new(self.major, self.minor, patch);
        self.get_header(version).await
    }
}
