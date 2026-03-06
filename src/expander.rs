use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::injector::Injector;
use crate::snippet::Snippet;

const MAX_BUFFER_SIZE: usize = 128;

pub struct Expander {
    buffer: String,
    snippets: Vec<Snippet>,
    injector: Injector,
}

impl Expander {
    pub fn new(
        mut snippets: Vec<Snippet>,
        pressed_keys: Arc<Mutex<HashSet<u16>>>,
        has_clipboard: bool,
    ) -> std::io::Result<Self> {
        snippets.sort_by(|left, right| {
            right
                .trigger_len()
                .cmp(&left.trigger_len())
                .then_with(|| left.trigger().cmp(right.trigger()))
        });

        let injector = Injector::new(pressed_keys, has_clipboard)?;

        Ok(Self {
            buffer: String::with_capacity(MAX_BUFFER_SIZE),
            snippets,
            injector,
        })
    }

    /// Process a typed character.
    pub fn push_char(&mut self, ch: char) {
        self.buffer.push(ch);

        if self.buffer.len() > MAX_BUFFER_SIZE {
            let excess = self.buffer.len() - MAX_BUFFER_SIZE;
            self.buffer.drain(..excess);
        }

        if let Some((trigger_len, replacement)) = self.find_match() {
            if let Err(error) = self.injector.expand(trigger_len, &replacement) {
                eprintln!("snippeto: failed to inject expansion: {error}");
            }
            self.buffer.clear();
        }
    }

    fn find_match(&self) -> Option<(usize, String)> {
        for snippet in &self.snippets {
            if self.buffer.ends_with(snippet.trigger()) {
                match snippet.render() {
                    Ok(replacement) => {
                        return Some((snippet.trigger_len(), replacement));
                    }
                    Err(error) => {
                        eprintln!("snippeto: failed to expand {}: {error}", snippet.trigger());
                        return None;
                    }
                }
            }
        }
        None
    }

    pub fn pop_char(&mut self) {
        self.buffer.pop();
    }

    pub fn clear_buffer(&mut self) {
        self.buffer.clear();
    }
}
