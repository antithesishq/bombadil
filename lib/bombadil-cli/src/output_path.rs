use anyhow::{Context, Result, bail};
use bombadil::model::TemplateRegistry;
use bombadil_trace::{TraceDriver, trace_driver};
use std::fs::File;
use std::path::{Path, PathBuf};

/// Validate a browser trace file, without interpreting directories.
pub fn resolve_browser_trace_file(path: &Path) -> Result<PathBuf> {
    match trace_driver(path)? {
        Some(TraceDriver::Browser) => Ok(path.to_path_buf()),
        Some(driver) => bail!(
            "{} holds a trace written by the {driver} driver, not a browser trace",
            path.display()
        ),
        None => bail!("no browser trace at {}", path.display()),
    }
}

/// Choose an unused file path in system temp, without retaining an empty
/// placeholder that would conflict with the writer's create_new.
pub fn resolve_output_path(output_path: &Option<PathBuf>) -> Result<PathBuf> {
    match output_path {
        Some(path) if path.is_dir() => bail!(
            "output path must name a trace file, not a directory: {}",
            path.display()
        ),
        Some(path) => Ok(path.clone()),
        None => {
            let file = tempfile::Builder::new()
                .prefix("bombadil-")
                .suffix(".bin")
                .tempfile()?;
            let path = file.path().to_path_buf();
            file.close()?;
            Ok(path)
        }
    }
}

/// The value of `first` if both are `Ok`, otherwise the error of `first`
/// before that of `second`. When both failed, the error of `second` is
/// logged.
pub fn or_logged<T>(first: Result<T>, second: Result<()>) -> Result<T> {
    match (first, second) {
        (Err(error), Err(error_second)) => {
            log::error!("{error_second:#}");
            Err(error)
        }
        (first, second) => second.and(first),
    }
}

/// Writes the `--action-templates` table.
pub fn write_action_templates(
    templates: Option<(&Path, &TemplateRegistry)>,
) -> Result<()> {
    let Some((path, registry)) = templates else {
        return Ok(());
    };
    (|| {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        registry.write(&mut File::create(path)?)
    })()
    .with_context(|| format!("writing action templates to {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn or_logged_returns_the_first_error_before_the_second() {
        let message = |result: Result<()>| result.unwrap_err().to_string();
        assert!(or_logged(Ok(()), Ok(())).is_ok());
        assert_eq!(message(or_logged(Err(anyhow!("first")), Ok(()))), "first");
        assert_eq!(
            message(or_logged(Ok(()), Err(anyhow!("second")))),
            "second"
        );
        assert_eq!(
            message(or_logged(Err(anyhow!("first")), Err(anyhow!("second")))),
            "first"
        );
    }

    #[test]
    fn arbitrary_files_and_directories_are_not_interchangeable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("arbitrary.data");
        let mut bytes = Vec::new();
        bombadil_trace::write_envelope(&mut bytes, TraceDriver::Browser)
            .unwrap();
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(resolve_browser_trace_file(&path).unwrap(), path);
        assert!(resolve_browser_trace_file(directory.path()).is_err());
        assert!(
            resolve_output_path(&Some(directory.path().to_path_buf())).is_err()
        );
        let default = resolve_output_path(&None).unwrap();
        assert!(!default.exists());
        assert_eq!(default.parent().unwrap(), std::env::temp_dir());
    }

    #[test]
    fn action_templates_are_written_into_a_new_directory() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("new/templates.jsonl");
        write_action_templates(Some((&path, &TemplateRegistry::default())))
            .unwrap();
        assert!(path.exists());
    }
}
