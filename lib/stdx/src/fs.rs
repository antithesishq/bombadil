use std::path::Path;

/// Whether `a` and `b` name the same existing file, through any spelling or
/// link.
pub fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (a.metadata(), b.metadata()) {
            return a.dev() == b.dev() && a.ino() == b.ino();
        }
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Whether `a` and `b` would be the same file, which need not exist: they
/// have the same name in the same directory.
pub fn same_location(a: &Path, b: &Path) -> bool {
    let directory = |path: &Path| {
        let parent = path.parent()?;
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        parent.canonicalize().ok()
    };
    match (a.file_name(), b.file_name(), directory(a), directory(b)) {
        (Some(a), Some(b), Some(a_directory), Some(b_directory)) => {
            a == b && a_directory == b_directory
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_location_sees_through_spellings_of_the_directory() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("trace.bin");
        assert!(same_location(&file, &file));
        assert!(same_location(&file, &directory.path().join("./trace.bin")));
        assert!(!same_location(&file, &directory.path().join("other.bin")));
        assert!(!same_location(
            &file,
            &directory.path().join("missing/trace.bin")
        ));
        assert!(same_location(
            Path::new("trace.bin"),
            Path::new("./trace.bin")
        ));
    }

    #[test]
    fn links_are_the_same_file_and_siblings_are_not() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("run-k2.bin");
        std::fs::write(&file, b"").unwrap();
        assert!(same_file(&file, &file));
        assert!(!same_file(&file, &directory.path().join("shrunk.bin")));
        let link = directory.path().join("hardlink");
        std::fs::hard_link(&file, &link).unwrap();
        assert!(same_file(&file, &link));
        #[cfg(unix)]
        {
            let link = directory.path().join("symlink");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert!(same_file(&file, &link));
        }
    }
}
