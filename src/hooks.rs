use crate::hooks_path;
use anyhow::{anyhow, bail, Result};
use indicatif::ProgressBar;
use std::{
    fmt,
    io::{BufRead, BufReader},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Debug, Copy, Clone)]
pub enum Hook {
    PostExtract,
}

impl fmt::Display for Hook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::PostExtract => "post-extract",
            }
        )
    }
}

impl Hook {
    fn get(hook: Self) -> Result<Option<PathBuf>> {
        let mut path: PathBuf = hooks_path()?;
        path.push(&hook.to_string());

        let mode = path.metadata()?.permissions().mode();

        if mode & 0o111 != 0 && path.exists() {
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }

    fn get_cmd(path: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new("bash");

        cmd.arg("-c")
            .arg(format!("{} {} 2>&1", path.display(), args.join(" ")))
            .stdout(Stdio::piped());

        cmd
    }

    pub fn exec(hook: Self, args: &[&str]) -> Result<()> {
        let Some(path) = Self::get(hook)? else {
            return Ok(());
        };

        let pb = ProgressBar::new_spinner();
        pb.set_message(format!("Running {hook} hook"));

        let mut cmd = Self::get_cmd(&path, args);

        let mut child = cmd.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Can't get stdout"))?;
        let reader = BufReader::new(stdout);

        for line in reader.lines() {
            let line = line?;
            pb.set_message(format!("Running {hook} hook: {line}"));
            pb.tick();
        }

        let status = child.wait()?;
        pb.finish_and_clear();

        if status.success() {
            Ok(())
        } else {
            bail!("Unable to execute the {hook} hook!")
        }
    }
}
