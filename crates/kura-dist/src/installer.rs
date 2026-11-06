//! The Inno Setup handoff.

use std::path::Path;
use std::process::{Command, Stdio};

use sha2::{Digest as _, Sha256};

use crate::{Error, Release, hex};

/// Silent, because the user already chose to install, and no restart, because
/// the application starts the installer itself. The user still sees Inno's
/// progress window.
const ARGS: [&str; 3] = ["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART"];

/// Checks the staged file once more and starts the installer. The app quits
/// right after this returns, so an installer that fails later is only visible
/// as the old version still running.
pub(crate) fn launch(release: &Release, artifact: &Path) -> Result<(), Error> {
    verify_file(release, artifact)?;
    installer_command(artifact)
        .spawn()
        .map(|_child| ())
        .map_err(|error| {
            Error::Install(format!(
                "{} could not be started: {error}",
                artifact.display()
            ))
        })
}

fn installer_command(artifact: &Path) -> Command {
    let mut command = Command::new(artifact);
    command
        .args(ARGS)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn verify_file(release: &Release, artifact: &Path) -> Result<(), Error> {
    let bytes = std::fs::read(artifact).map_err(|error| {
        Error::Install(format!("{} could not be read: {error}", artifact.display()))
    })?;
    if !release
        .sha256
        .eq_ignore_ascii_case(&hex(&Sha256::digest(&bytes)))
    {
        return Err(Error::Signature(
            "the staged installer no longer matches the manifest".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Version;

    fn release(sha256: String) -> Release {
        Release::new(
            Version::new(1, 2, 3),
            "https://example.invalid/Kura-1.2.3-setup.exe".to_owned(),
            10,
            sha256,
            String::new(),
        )
    }

    #[test]
    fn the_installer_runs_silently() {
        let command = installer_command(Path::new("Kura-1.2.3-setup.exe"));
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]);
    }

    #[test]
    fn a_changed_installer_is_refused_before_it_runs() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let artifact = dir.path().join("Kura-1.2.3-setup.exe");
        std::fs::write(&artifact, b"not the published bytes").expect("write the artifact");
        let digest = hex(&Sha256::digest(b"the published bytes"));

        assert!(matches!(
            launch(&release(digest), &artifact),
            Err(Error::Signature(_))
        ));
    }
}
