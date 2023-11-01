use crate::hooks_path;
use anyhow::Result;
use indicatif::ProgressBar;
use std::{
    fmt,
    io::{BufRead, BufReader},
    path::PathBuf,
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
        let name = hook.to_string();
        path.push(&name);

        if path.exists() {
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }

    pub fn exec(hook: Self, args: &[&str]) -> Result<()> {
        if let Some(path) = Self::get(hook)? {
            let pb = ProgressBar::new_spinner();
            pb.set_message(format!("Running {hook} hook"));

            let mut cmd = Command::new("bash");
            cmd.arg("-c")
                .arg(format!("{} {} 2>&1", path.display(), args.join(" ")));
            cmd.stdout(Stdio::piped());

            let mut child = cmd.spawn()?;
            let stdout = child.stdout.take().expect("Can't get stdout");
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
                anyhow::bail!("Unable to execute the {hook} hook!")
            }
        } else {
            Ok(())
        }
    }
}
