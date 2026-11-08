use anyhow::{Context, Result};
use bombadil::model::{self, TemplateRegistry};
use clap::Args;
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    thread::{self, JoinHandle},
};

#[derive(Args, Clone)]
pub struct ActionTemplatesOptions {
    /// Write the table of action templates that trace `actions_available`
    /// keys refer to (key, template as JSON) to this file, as JSON lines.
    /// A template is appended when it is first available. Without it no
    /// table is written.
    #[arg(long = "action-templates", value_name = "FILE")]
    pub action_templates_path: Option<PathBuf>,
}

/// The `--action-templates` file, created before the run so that a path that
/// cannot be written fails the command at its start. A thread appends each
/// template as it is first seen, so a killed run leaves its table.
pub struct ActionTemplates {
    registry: TemplateRegistry,
    writer: JoinHandle<io::Result<()>>,
    path: PathBuf,
}

impl ActionTemplates {
    pub fn create(options: &ActionTemplatesOptions) -> Result<Option<Self>> {
        let Some(path) = options.action_templates_path.as_deref() else {
            return Ok(None);
        };
        let file = create_file(path).with_context(|| {
            format!("creating action templates file {}", path.display())
        })?;
        let (registry, rows_new) = TemplateRegistry::with_channel();
        let writer = thread::Builder::new()
            .name("action-templates".into())
            .spawn(move || model::write_template_rows(rows_new, file))?;
        Ok(Some(ActionTemplates {
            registry,
            writer,
            path: path.to_path_buf(),
        }))
    }

    pub fn registry(&self) -> &TemplateRegistry {
        &self.registry
    }

    /// Waits for the table to be written.
    pub fn finish(self) -> Result<()> {
        self.registry.close();
        self.writer
            .join()
            .expect("action templates writer panicked")
            .with_context(|| {
                format!("writing action templates to {}", self.path.display())
            })
    }
}

fn create_file(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    File::create(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bombadil::tree::Tree;
    use bombadil_browser::browser::actions::BrowserActionTemplate;

    #[test]
    fn rows_are_in_the_file_before_the_run_ends() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("new/templates.jsonl");
        let templates = ActionTemplates::create(&ActionTemplatesOptions {
            action_templates_path: Some(path.clone()),
        })
        .unwrap()
        .unwrap();
        assert!(path.exists());

        let tree = Tree::Leaf {
            value: BrowserActionTemplate::Back,
        };
        templates.registry().record(&tree.leaves()).unwrap();
        let row = "{\"key\":\"60e839e3a8241847\",\"template\":\"Back\"}\n";
        while std::fs::read_to_string(&path).unwrap() != row {
            thread::yield_now();
        }

        templates.finish().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), row);
    }
}
