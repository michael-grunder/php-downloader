use crate::downloads::DownloadInfo;

use colored::Colorize;
use serde_json::to_string_pretty;

pub trait Viewer: Send + Sync {
    fn display(&self, data: &[DownloadInfo]);
}

struct CliViewer;
struct JsonViewer;

impl<T: Into<u64>> ToHumanSize for T {
    fn to_human_size(self) -> String {
        Self::to_human_size_fmt(self.into())
    }
}

pub trait ToHumanSize {
    fn to_human_size(self) -> String;

    fn to_human_size_fmt(v: u64) -> String {
        let (val, unit) = Self::to_human_size_impl(v);
        format!("{val:.2} {unit}")
    }

    fn to_human_size_impl(v: u64) -> (f64, &'static str) {
        const KB: f64 = 1024.0;
        const MB: f64 = KB * 1024.0;
        const GB: f64 = MB * 1024.0;
        const TB: f64 = GB * 1024.0;

        #[allow(clippy::cast_precision_loss)]
        let v = v as f64;

        if v < KB {
            (v, "B")
        } else if v < MB {
            (v / KB, "KB")
        } else if v < GB {
            (v / MB, "MB")
        } else if v < TB {
            (v / GB, "GB")
        } else {
            (v / TB, "TB")
        }
    }
}

impl Viewer for CliViewer {
    fn display(&self, urls: &[DownloadInfo]) {
        // Calculating the maximum lengths of each field in a more idiomatic way
        let max_lens = urls.iter().fold([0, 0, 0, 0], |mut acc, url| {
            acc[0] = acc[0].max(url.version.to_string().len());
            acc[1] = acc[1].max(url.size.to_human_size().len());
            acc[2] = acc[2].max(url.date_string().len());
            acc[3] = acc[3].max(url.location.len());
            acc
        });

        // Printing each url with fields aligned based on their maximum lengths
        // "{:<width0$} \u{2502} {:<width1$} {:>width2$} \u{2192} {:<width3$}",
        for url in urls {
            println!(
                "{:<width0$}\t{:<width1$}\t{:>width2$}\t{:<width3$}",
                url.version.to_string().bold(),
                url.size.to_human_size(),
                url.date_string(),
                url.location,
                width0 = max_lens[0],
                width1 = max_lens[1],
                width2 = max_lens[2],
                width3 = max_lens[3],
            );
        }
    }
}

impl Viewer for JsonViewer {
    fn display(&self, urls: &[DownloadInfo]) {
        let s = to_string_pretty(urls).unwrap_or_else(|_| String::from("Error generating JSON"));
        println!("{s}");
    }
}

pub fn get_viewer(json: bool) -> Box<dyn Viewer + Send + Sync> {
    if json {
        Box::new(JsonViewer)
    } else {
        Box::new(CliViewer)
    }
}
