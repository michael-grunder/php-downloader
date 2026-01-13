use crate::config::Config;
use anyhow::{Result, anyhow};
use indicatif::ProgressBar;
use std::{
    fmt,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tempfile::NamedTempFile;

#[derive(Debug)]
pub struct ScriptResult {
    pub status: i32,
    pub output: Vec<String>,
}

#[derive(Debug, Copy, Clone)]
pub enum Hook {
    PostExtract,
    Configure,
    Make,
}

impl fmt::Display for Hook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str(),)
    }
}

impl ScriptResult {
    const fn new() -> Self {
        Self {
            status: 0,
            output: vec![],
        }
    }

    fn push(&mut self, line: &str) {
        self.output.push(line.to_string());
    }

    const fn set_status(&mut self, status: i32) {
        self.status = status;
    }

    pub fn save(&self) -> Result<PathBuf> {
        let mut tmp = NamedTempFile::new()?;

        for line in &self.output {
            writeln!(tmp, "{line}")?;
        }

        let path = tmp.path().to_owned();
        tmp.persist(&path)?;

        Ok(path)
    }
}

impl Hook {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configure => "configure",
            Self::Make => "make",
            Self::PostExtract => "post-extract",
        }
    }

    fn get(hook: Self) -> Result<Option<PathBuf>> {
        let mut path: PathBuf = Config::hooks_path()?;
        path.push(hook.to_string());

        let mode = path.metadata()?.permissions().mode();

        if mode & 0o111 != 0 && path.exists() {
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }

    fn get_cmd(path: &Path, working_dir: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new("bash");

        cmd.arg("-c")
            .current_dir(working_dir)
            .arg(format!("{} {} 2>&1", path.display(), args.join(" ")))
            .stdout(Stdio::piped());

        cmd
    }

    pub fn exec<P: AsRef<Path>>(
        hook: Self,
        working_dir: P,
        args: &[&str],
    ) -> Result<ScriptResult> {
        let mut res = ScriptResult::new();

        let Some(path) = Self::get(hook)? else {
            return Ok(res);
        };

        let pb = ProgressBar::new_spinner();
        pb.set_message(format!("Running {hook} hook"));

        let mut cmd = Self::get_cmd(&path, working_dir.as_ref(), args);

        let mut child = cmd.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Can't get stdout"))?;
        let reader = BufReader::new(stdout);

        for line in reader.lines() {
            let line = line?;
            res.push(&line);
            pb.set_message(format!("Running {hook} hook: {line}"));
            pb.tick();
        }

        let status = child.wait()?;
        pb.finish_and_clear();
        res.set_status(status.code().unwrap_or(0));

        Ok(res)
    }
}
