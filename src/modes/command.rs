use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use lazy_static::lazy_static;
use maplit::hashmap;

use crate::keymap::KeyMap;
use crate::modes::{
    mode::{Mode, ModeTransition},
    normal::Normal,
};
use crate::BuffrCollection;

pub struct Command {
    pub command: String,
    pub cursor: usize,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Action {
    RemoveLast,
    RemoveThis,
    CursorLeft,
    CursorRight,
    Finish,
    Cancel,
}

fn default_maps() -> KeyMap<Action> {
    KeyMap {
        maps: keys!(
            (key KeyCode::Backspace => Action::RemoveLast),
            (key KeyCode::Delete => Action::RemoveThis),
            (key KeyCode::Enter => Action::Finish),
            (key KeyCode::Esc => Action::Cancel),
            (key KeyCode::Left => Action::CursorLeft),
            (key KeyCode::Right => Action::CursorRight)
        ),
    }
}

mod cmd {
    use super::*;
    use crate::modes::mode::DirtyBytes;
    use crate::modes::quitting;

    pub fn quit(buf: &mut BuffrCollection, _: &str) -> ModeTransition {
        if buf.iter().any(|x| x.dirty && x.path.is_some()) {
            ModeTransition::new_mode_and_info(
                Normal::new(),
                "unsaved changes! Run :wq or :q! instead.".into(),
            )
        } else {
            ModeTransition::new_mode(quitting::Quitting {})
        }
    }

    pub fn force_quit(_: &mut BuffrCollection, _: &str) -> ModeTransition {
        ModeTransition::new_mode(quitting::Quitting {})
    }

    pub fn write(buf: &mut BuffrCollection, filename: &str) -> ModeTransition {
        let path = if filename.is_empty() {
            buf.current().path.as_deref()
        } else {
            Some(std::path::Path::new(&filename))
        };

        if let Some(path) = path {
            if let Err(e) = fs::write(&path, buf.current().data.slice_to_cow(..)) {
                return ModeTransition::new_mode_and_info(
                    Normal::new(),
                    format!("write failed: {}", e),
                );
            }

            let owned_path = path.to_owned();
            let buf_mut = buf.current_mut();
            buf_mut.dirty = false;
            buf_mut.update_path_if_missing(owned_path);
            ModeTransition::new_mode(Normal::new())
        } else {
            ModeTransition::new_mode_and_info(Normal::new(), "current_buffer has no path".into())
        }
    }

    pub fn write_all(buffr_collection: &mut BuffrCollection, _: &str) -> ModeTransition {
        for buf in buffr_collection.iter_mut() {
            if let Some(path) = buf.path.as_ref() {
                if let Err(e) = fs::write(&path, buf.data.slice_to_cow(..)) {
                    return ModeTransition::new_mode_and_info(
                        Normal::new(),
                        format!("write failed: {}", e),
                    );
                }
                buf.dirty = false;
            }
        }
        ModeTransition::new_mode(Normal::new())
    }

    pub fn write_quit(buffr_collection: &mut BuffrCollection, _: &str) -> ModeTransition {
        for buf in buffr_collection.iter_mut() {
            if let Some(path) = buf.path.as_ref() {
                if let Err(e) = fs::write(&path, buf.data.slice_to_cow(..)) {
                    return ModeTransition::new_mode_and_info(
                        Normal::new(),
                        format!("write failed: {}", e),
                    );
                }
                buf.dirty = false;
            }
        }
        ModeTransition::new_mode(quitting::Quitting {})
    }

    pub fn edit(buffr_collection: &mut BuffrCollection, filename: &str) -> ModeTransition {
        let result = buffr_collection.switch_current_buffer(filename);
        if let Err(e) = result {
            return ModeTransition::new_mode_and_info(Normal::new(), format!("{}", e));
        }
        ModeTransition::new_mode_and_dirty(Normal::new(), DirtyBytes::ChangeLength)
    }

    pub fn delete_current_buffer(buffr_collection: &mut BuffrCollection, _: &str) -> ModeTransition {
        if buffr_collection.current().dirty && buffr_collection.current().path.is_some() {
            return ModeTransition::new_mode_and_info(
                Normal::new(),
                "current_buffer is dirty, use :db! if you're sure".to_string(),
            );
        }
        buffr_collection.delete_current();
        ModeTransition::new_mode_and_dirty(Normal::new(), DirtyBytes::ChangeLength)
    }

    pub fn force_delete_current_buffer(buffr_collection: &mut BuffrCollection, _: &str) -> ModeTransition {
        buffr_collection.delete_current();
        ModeTransition::new_mode_and_dirty(Normal::new(), DirtyBytes::ChangeLength)
    }
}

type CommandHandler = fn(&mut BuffrCollection, &str) -> ModeTransition;

macro_rules! make_commands {
    ($($string:tt => $cmd:ident,)*) => {
        hashmap![
            $($string.to_string() => (cmd::$cmd as CommandHandler),)*
        ]
    }
}

fn default_commands() -> HashMap<String, CommandHandler> {
    make_commands![
        "q" => quit,
        "quit" => quit,
        "q!" => force_quit,
        "quit!" => force_quit,
        "w" => write,
        "write" => write,
        "wq" => write_quit,
        "wa" => write_all,
        "write-all" => write_all,
        "e" => edit,
        "edit" => edit,
        "db" => delete_current_buffer,
        "delete-current_buffer" => delete_current_buffer,
        "db!" => force_delete_current_buffer,
        "delete-current_buffer!" => force_delete_current_buffer,
    ]
}

lazy_static! {
    static ref DEFAULT_MAPS: KeyMap<Action> = default_maps();
    static ref DEFAULT_COMMANDS: HashMap<String, CommandHandler> = default_commands();
}

impl Command {
    pub fn new() -> Command {
        Command {
            cursor: 0,
            command: String::new(),
        }
    }

    fn finish(&self, buffr_collection: &mut BuffrCollection) -> ModeTransition {
        let (name, rest) = self
            .command
            .split_at(self.command.find(' ').unwrap_or_else(|| self.command.len()));
        if let Some(handler) = DEFAULT_COMMANDS.get(name) {
            handler(buffr_collection, if rest.is_empty() { rest } else { &rest[1..] })
        } else {
            ModeTransition::new_mode_and_info(Normal::new(), format!("Unknown command {}", name))
        }
    }
}

impl Mode for Command {
    fn name(&self) -> Cow<'static, str> {
        "COMMAND".into()
    }

    fn transition(&self, evt: &Event, buffr_collection: &mut BuffrCollection, _: usize) -> Option<ModeTransition> {
        if let Some(action) = DEFAULT_MAPS.event_to_action(evt) {
            let mut cursor = self.cursor;
            let mut command = self.command.to_owned();

            match action {
                Action::RemoveLast if cursor != 0 => {
                    command.remove(cursor - 1);
                    cursor -= 1;
                }
                Action::RemoveLast => return Some(ModeTransition::None),
                Action::RemoveThis => {
                    command.remove(cursor);
                } // Don't move the cursor
                Action::CursorLeft if cursor != 0 => {
                    cursor -= 1;
                }
                Action::CursorLeft => {}
                Action::CursorRight if cursor < command.len() => {
                    cursor += 1;
                }
                Action::CursorRight => {}
                Action::Cancel => return Some(ModeTransition::new_mode(Normal::new())),
                Action::Finish => return Some(self.finish(buffr_collection)),
            }
            Some(ModeTransition::new_mode(Command { command, cursor }))
        } else if let Event::Key(KeyEvent {
            code: KeyCode::Char(ch),
            modifiers,
        }) = evt
        {
            if !(*modifiers & !KeyModifiers::SHIFT).is_empty() {
                return None;
            }
            let mut command = self.command.to_owned();
            let mut cursor = self.cursor;
            command.insert(cursor, *ch);
            cursor += 1;
            Some(ModeTransition::new_mode(Command { command, cursor }))
        } else {
            None
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
