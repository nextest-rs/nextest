use crate::test_output::CaptureStrategy;
use std::process::Stdio;
use tokio::process::{Child as TokioChild, ChildStdout as Pipe};

cfg_if::cfg_if! {
    if #[cfg(unix)] {
        #[path = "unix.rs"]
        mod unix;
        use unix as os;
    } else if #[cfg(windows)] {
        #[path = "windows.rs"]
        mod windows;
        use windows as os;
    } else {
        compile_error!("unsupported target platform");
    }
}

pub enum Output {
    Split { stdout: Pipe, stderr: Pipe },
    Combined(Pipe),
}

pub struct Child {
    pub child: TokioChild,
    pub output: Option<Output>,
}

impl std::ops::Deref for Child {
    type Target = TokioChild;

    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl std::ops::DerefMut for Child {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

pub(super) fn spawn(
    mut cmd: std::process::Command,
    strategy: CaptureStrategy,
) -> std::io::Result<Child> {
    cmd.stdin(Stdio::null());

    let state: Option<os::State> = match strategy {
        CaptureStrategy::None => None,
        CaptureStrategy::Split => {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            None
        }
        CaptureStrategy::Combined => Some(os::setup_io(&mut cmd)?),
    };

    let mut cmd: tokio::process::Command = cmd.into();
    let mut child = cmd.spawn()?;

    let output = match strategy {
        CaptureStrategy::None => None,
        CaptureStrategy::Split => {
            let stdout = child.stdout.take().expect("stdout was set");
            let stderr = child.stderr.take().expect("stderr was set");
            let stderr = os::stderr_to_stdout(stderr)?;

            Some(Output::Split { stdout, stderr })
        }
        CaptureStrategy::Combined => Some(Output::Combined(os::state_to_stdout(
            state.expect("state was set"),
        )?)),
    };

    Ok(Child { child, output })
}
