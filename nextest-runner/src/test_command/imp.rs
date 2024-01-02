use crate::test_output::CaptureStrategy;
use std::process::Stdio;
use tokio::{
    fs::File,
    process::{self as proc, Child as TokioChild},
};

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
    Split {
        stdout: proc::ChildStdout,
        stderr: proc::ChildStderr,
    },
    Combined(File),
}

pub struct Child {
    pub child: TokioChild,
    pub output: Option<Output>,
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

            Some(Output::Split { stdout, stderr })
        }
        CaptureStrategy::Combined => Some(Output::Combined(
            std::fs::File::from(state.expect("state was set").ours).into(),
        )),
    };

    Ok(Child { child, output })
}
