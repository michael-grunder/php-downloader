use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use futures::future::join_all;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use serde::{de, ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer};
use std::{
    cmp::Ordering, fmt, fs, io::Write, os::unix::fs::PermissionsExt, path::Path,
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
    GZ,
    BZ,
    XZ,
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
    pub patch: Option<u8>,
    pub rc: Option<VersionModifier>,
}

impl std::default::Default for Extension {
    fn default() -> Self {
        Self::BZ
    }
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
        match &*s.to_lowercase() {
            "alpha" => Ok(Self::Alpha),
            "beta" => Ok(Self::Beta),
            "rc" => Ok(Self::RC),
            _ => Err(anyhow!("Don't understand version modifier '{s}'")),
        }
    }
}

impl VersionModifier {
    fn split_at_digit(input: &str) -> Option<(&str, &str)> {
        for (index, char) in input.char_indices() {
            if char.is_numeric() {
                let (start, end) = input.split_at(index);
                return Some((start, end));
            }
        }
        None
    }

    fn from_patch(s: &str) -> Result<(Option<Self>, Option<u8>)> {
        let s = s.find(|c: char| !c.is_ascii_digit()).map_or(s, |i| &s[i..]);

        if let Some((modifier, patch)) = Self::split_at_digit(s) {
            let modifier = (!modifier.is_empty())
                .then(|| Self::from_str(modifier))
                .transpose()?;

            let patch = (!patch.is_empty()).then(|| patch.parse()).transpose()?;

            Ok((modifier, patch))
        } else {
            Err(anyhow!(format!("Unable to parse patch '{s}'")))
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

    //fn get_age(date: &DateTime<Utc>) -> String {
    //    let now = Utc::now();
    //    let duration = now.signed_duration_since(date);

    //    let years = duration.num_days() / 365;
    //    let remaining_days_year = duration.num_days() % 365;

    //    let months = remaining_days_year / 30;
    //    let remaining_days_month = remaining_days_year % 30;

    //    let days = remaining_days_month;

    //    let hours = duration.num_hours() % 24;
    //    let minutes = duration.num_minutes() % 60;

    //    let parts = if years > 0 {
    //        vec![(years, "year"), (months, "month")]
    //    } else if months > 0 {
    //        vec![(months, "month"), (days, "day")]
    //    } else if days > 0 {
    //        vec![(days, "day"), (hours, "hour")]
    //    } else {
    //        vec![(hours, "hour"), (minutes, "minute")]
    //    };

    //    parts
    //        .into_iter()
    //        .filter(|(v, _)| v > &0)
    //        .map(|(v, ident)| format!("{v} {ident}{}", if v > 1 { "s" } else { "" }))
    //        .collect::<Vec<String>>()
    //        .join(" ")
    //}

    pub async fn download_to_file(&self, dst: &Path) -> Result<()> {
        let mut tmp = NamedTempFile::new()?;

        let mut perms = fs::metadata(tmp.path())?.permissions();
        perms.set_mode(0o644);
        fs::set_permissions(tmp.path(), perms)?;

        self.download(tmp.as_file_mut()).await?;

        tmp.persist(dst)?;

        Ok(())
    }

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
            writer.write_all(&chunk)?;
        }

        pb.finish_with_message("download completed");
        Ok(())
    }
}

impl Version {
    pub const fn from_major_minor(major: u8, minor: u8) -> Self {
        Self::new(major, minor, None, None)
    }

    pub const fn from_major_minor_patch(major: u8, minor: u8, patch: u8) -> Self {
        Self::new(major, minor, Some(patch), None)
    }

    pub const fn new(major: u8, minor: u8, patch: Option<u8>, rc: Option<VersionModifier>) -> Self {
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

    pub async fn resolve_latest(&mut self, dl: &DownloadList) -> Result<()> {
        if self.patch.is_none() {
            *self = dl
                .latest()
                .await?
                .ok_or_else(|| anyhow!("Failed to resolve the latest patch for version {}", self))?
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
                "{}.{}.0{}{}",
                self.major,
                self.minor,
                rc,
                self.patch.unwrap_or(0)
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

impl fmt::Display for VersionModifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = match self {
            Self::Alpha => "alpha",
            Self::Beta => "beta",
            Self::RC => "RC",
        };

        write!(f, "{v}")
    }
}

impl VersionModifier {
    pub fn variants() -> Vec<Self> {
        vec![Self::Alpha, Self::Beta, Self::RC]
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

    async fn get_header(&self, version: Version) -> Result<Option<DownloadInfo>> {
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
        Box::new(
            (0..31).map(|patch| Version::from_major_minor_patch(self.major, self.minor, patch)),
        )
    }

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

    pub async fn latest(&self) -> Result<Option<DownloadInfo>> {
        let mut urls = self.list().await?;
        Ok(urls.pop())
    }

    pub async fn get(&self, version: Version) -> Result<Option<DownloadInfo>> {
        self.get_header(version).await
    }
}
