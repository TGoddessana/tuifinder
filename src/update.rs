use std::{error::Error, process::Command};

pub fn run() -> Result<(), Box<dyn Error>> {
    let executable = std::env::current_exe()?.canonicalize()?;
    if executable
        .file_name()
        .is_none_or(|name| name != "tuifinder")
    {
        return Err(
            "Self-update expects a binary named tuifinder; re-run install.sh instead".into(),
        );
    }
    let repo = std::env::var("TUIFINDER_REPO")
        .ok()
        .or_else(|| option_env!("TUIFINDER_REPO").map(str::to_owned))
        .unwrap_or_else(|| "TGoddessana/tuifinder".into());
    let status = Command::new("sh")
        .arg("-c")
        .arg(include_str!("../install.sh"))
        .arg("tuifinder-installer")
        .args(["--repo", &repo, "--dir"])
        .arg(
            executable
                .parent()
                .ok_or("Cannot locate installation directory")?,
        )
        .status()?;
    if !status.success() {
        return Err("Update failed; see installer error above".into());
    }
    Ok(())
}
