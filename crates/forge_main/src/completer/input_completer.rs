use std::path::PathBuf;
use std::sync::Arc;

use forge_walker::Walker;
use reedline::{Completer, Suggestion};

use crate::completer::search_term::SearchTerm;
use crate::completer::CommandCompleter;
use crate::model::ForgeCommandManager;

#[derive(Clone)]
pub struct InputCompleter {
    walker: Walker,
    command: CommandCompleter,
}

impl InputCompleter {
    pub fn new(cwd: PathBuf, command_manager: Arc<ForgeCommandManager>) -> Self {
        let walker = Walker::max_all().cwd(cwd).skip_binary(true);
        Self { walker, command: CommandCompleter::new(command_manager) }
    }
}

impl Completer for InputCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        if line.starts_with("/") {
            // if the line starts with '/' it's probably a command, so we delegate to the
            // command completer.
            let result = self.command.complete(line, pos);
            if !result.is_empty() {
                return result;
            }
        }

        if let Some(query) = SearchTerm::new(line, pos).process() {
            let files = self.walker.get_blocking().unwrap_or_default();
            files
                .into_iter()
                .filter(|file| !file.is_dir())
                .filter_map(|file| {
                    if let Some(file_name) = file.file_name.as_ref() {
                        let file_name_lower = file_name.to_lowercase();
                        let query_lower = query.term.to_lowercase();
                        if file_name_lower.contains(&query_lower) {
                            let name_md_fmt = format!("[{}]", file_name);
                            let path_without_filename = file.path.to_string();
                            let path_without_filename = path_without_filename.strip_suffix(file_name).unwrap_or(&path_without_filename);
                            Some(Suggestion {
                                description: None,
                                value: name_md_fmt,
                                style: None,
                                extra: None,
                                span: query.span,
                                append_whitespace: true,
                                prefix_string: Some(path_without_filename.to_string()),
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            vec![]
        }
    }
}
