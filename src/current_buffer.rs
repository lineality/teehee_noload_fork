use xi_rope::Interval;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::byte_rope::*;
use super::history::History;
use crate::modes::mode::DirtyBytes;
use crate::selection::{SelRegion, Selection};

use std::fs::File;
use std::io::SeekFrom;
use std::io::Seek;
use std::io::Read;

use xi_rope::tree::TreeBuilder;
use xi_rope::Delta;

use std::fs::OpenOptions;
use std::io::Write;
use std::env;
use std::time::{
    SystemTime, 
    UNIX_EPOCH
};

const DEBUG_LOG: bool = false;

fn debug_log(message: &str) {
    /*
    use std::fs::OpenOptions;
    use std::io::Write;    
    use std::env;
    use std::time::{
        SystemTime, 
        UNIX_EPOCH
    };
    */
    if DEBUG_LOG {
        if let Ok(cwd) = env::current_dir() {
            let log_path = cwd.join("teehee_debug.log");

            // Check if path exists and is writable
            if log_path.exists() {
                // If the path exists, attempt to open the file for writing
                if let Ok(mut file) = OpenOptions::new()
                    .write(true)
                    .append(true)
                    .open(&log_path) {

                    // If the file is opened successfully, get the current time and write the message to the file
                    if let Ok(timestamp) = SystemTime::now().duration_since(UNIX_EPOCH) {
                        let _ = writeln!(file, "[{}] {}", timestamp.as_secs(), message);
                    }
                }
            }
        }
    }
}


#[derive(Debug, PartialEq, Clone, Copy, Eq, Hash)]
pub enum OverflowSelectionStyle {
    Cursor,
    Tail,
    CursorTail,
}


#[derive(Default)]
pub struct CurrentBuffer {
    pub path: Option<PathBuf>,
    pub data: Rope,
    pub selection: Selection,
    pub registers: HashMap<char, Vec<Vec<u8>>>,
    pub dirty: bool,
    pub history: History,
    pub data_start_offset: usize,
}

// in current_buffers.rs
impl CurrentBuffer {
    pub fn from_data_and_path(data: Vec<u8>, path: Option<impl Into<PathBuf>>) -> CurrentBuffer {
        CurrentBuffer {
            data: data.into(),
            selection: Selection::new(),
            registers: HashMap::new(),
            dirty: false,
            path: path.map(Into::into),
            history: History::new(),
            data_start_offset: 0,  // Initialize at 0
        }
    }
    
    pub fn data_start_offset(&self) -> usize {
        self.data_start_offset
    }
    
    pub fn load_next_chunk(&mut self, chunk_size: usize) -> Result<bool, std::io::Error> {
        debug_log("Entering load_next_chunk");
        
        if let Some(path) = &self.path {
            debug_log(&format!("Loading from path: {:?}", path));
            let mut file = File::open(path)?;
            
            // Log current position and file size
            let file_size = file.metadata()?.len();
            debug_log(&format!("File size: {}, Current buffer size: {}", 
                file_size, self.data.len()));
            
            // Seek to the end of current data
            file.seek(SeekFrom::Start(self.data.len() as u64))?;
            debug_log(&format!("Seeking to position: {}", self.data.len()));
            
            // Read next chunk
            let mut next_chunk = vec![0; chunk_size];
            let bytes_read = file.read(&mut next_chunk)?;
            debug_log(&format!("Bytes read: {}", bytes_read));
            
            if bytes_read > 0 {
                debug_log("Creating new rope from chunk");
                // Create new rope from chunk
                let mut builder = TreeBuilder::new();
                builder.push_leaf(Bytes(next_chunk[..bytes_read].to_vec()));
                let chunk_node = builder.build();
                
                // Create delta for appending
                let delta = Delta::simple_edit(
                    Interval::new(self.data.len(), self.data.len()),
                    chunk_node,
                    self.data.len()
                );
                
                // Apply the delta to append the new chunk
                let old_size = self.data.len();
                self.data = self.data.apply_delta(&delta);
                debug_log(&format!("Buffer size changed: {} -> {}", 
                    old_size, self.data.len()));
                
                Ok(true)
            } else {
                debug_log("No more data to read");
                Ok(false)  // No more data to read
            }
        } else {
            debug_log("No file path available");
            Ok(false)  // No file path
        }
    }

    pub fn name(&self) -> String {
        if let Some(path) = &self.path {
            format!("{}", path.display())
        } else {
            "*scratch*".to_string()
        }
    }

    pub fn map_selections(&mut self, mut f: impl FnMut(SelRegion) -> Vec<SelRegion>) -> DirtyBytes {
        let mut invalidated_ranges = Vec::new();
        self.selection.map_selections(|region| {
            invalidated_ranges.push(Interval::from(region.min()..=region.max()));
            let new = f(region);
            for new_reg in new.iter() {
                invalidated_ranges.push(Interval::from(new_reg.min()..=new_reg.max()));
            }
            new
        });
        invalidated_ranges.sort_by(|a, b| a.start.cmp(&b.start));

        let mut disjoint_invalidated_ranges = Vec::new();
        for r in invalidated_ranges {
            if disjoint_invalidated_ranges.is_empty() {
                disjoint_invalidated_ranges.push(r);
                continue;
            }
            let last = disjoint_invalidated_ranges.last().unwrap();
            if last.contains(r.start) {
                *disjoint_invalidated_ranges.last_mut().unwrap() = last.union(r);
                continue;
            }
            disjoint_invalidated_ranges.push(r);
        }
        DirtyBytes::ChangeInPlace(disjoint_invalidated_ranges)
    }

    fn apply_delta_to_current_buffer(&mut self, delta: RopeDelta, is_final: bool) {
        let next_data = self.data.apply_delta(&delta);
        if is_final {
            self.history
                .perform_final(&self.data, delta, self.selection.clone());
        } else {
            self.history
                .perform_partial(&self.data, delta, &self.selection);
        }
        self.data = next_data;
        self.dirty = true;
    }

    pub fn apply_delta(&mut self, delta: RopeDelta) -> DirtyBytes {
        let max_len = self.data.len();
        self.apply_delta_to_current_buffer(delta.clone(), true);
        self.selection.apply_delta(&delta, max_len);

        DirtyBytes::ChangeLength
    }

    pub fn apply_delta_offset_carets(
        &mut self,
        delta: RopeDelta,
        caret_offset: isize,
        tail_offset: isize,
    ) -> DirtyBytes {
        let max_len = self.data.len();
        self.apply_delta_to_current_buffer(delta.clone(), true);
        self.selection
            .apply_delta_offset_carets(&delta, caret_offset, tail_offset, max_len);

        DirtyBytes::ChangeLength
    }

    pub fn apply_incomplete_delta(&mut self, delta: RopeDelta) -> DirtyBytes {
        let max_len = self.data.len();
        self.apply_delta_to_current_buffer(delta.clone(), false);
        self.selection.apply_delta(&delta, max_len);

        DirtyBytes::ChangeLength
    }

    pub fn apply_incomplete_delta_offset_carets(
        &mut self,
        delta: RopeDelta,
        caret_offset: isize,
        tail_offset: isize,
    ) -> DirtyBytes {
        let max_len = self.data.len();
        self.apply_delta_to_current_buffer(delta.clone(), false);
        self.selection
            .apply_delta_offset_carets(&delta, caret_offset, tail_offset, max_len);

        DirtyBytes::ChangeLength
    }

    pub fn commit_delta(&mut self) {
        self.history.commit_partial();
    }

    pub fn perform_undo(&mut self) -> Option<DirtyBytes> {
        if let Some((undo_delta, old_selection)) =
            self.history.undo(&self.data, self.selection.clone())
        {
            self.selection = old_selection;
            self.data = self.data.apply_delta(&undo_delta);
            self.dirty = true;
            Some(DirtyBytes::ChangeLength)
        } else {
            None
        }
    }

    pub fn perform_redo(&mut self) -> Option<DirtyBytes> {
        if let Some((redo_delta, old_selection)) =
            self.history.redo(&self.data, self.selection.clone())
        {
            self.selection = old_selection;
            self.data = self.data.apply_delta(&redo_delta);
            self.dirty = true;
            Some(DirtyBytes::ChangeLength)
        } else {
            None
        }
    }

    fn switch_main_sel(&mut self, f: impl FnOnce(&mut Selection)) -> DirtyBytes {
        let old_main_sel_interval = self.selection.main().into();
        f(&mut self.selection);
        let new_main_sel_interval = self.selection.main().into();
        DirtyBytes::ChangeInPlace(vec![old_main_sel_interval, new_main_sel_interval])
    }

    fn modify_sels_in_place(&mut self, f: impl FnOnce(&mut Selection)) -> DirtyBytes {
        let dirty =
            DirtyBytes::ChangeInPlace(self.selection.iter().copied().map(Into::into).collect());
        f(&mut self.selection);

        dirty
    }

    pub fn remove_selection(&mut self, index: usize) -> DirtyBytes {
        self.modify_sels_in_place(|sel| sel.remove(index % sel.len()))
    }
    pub fn retain_selection(&mut self, index: usize) -> DirtyBytes {
        self.modify_sels_in_place(|sel| sel.retain(index % sel.len()))
    }
    pub fn select_next(&mut self, count: usize) -> DirtyBytes {
        self.switch_main_sel(|sel| sel.select_next(count))
    }
    pub fn select_prev(&mut self, count: usize) -> DirtyBytes {
        self.switch_main_sel(|sel| sel.select_prev(count))
    }

    pub fn yank_selections(&mut self, reg: char) {
        if self.data.is_empty() {
            self.registers
                .insert(reg, vec![vec![]; self.selection.len()]);
            return;
        }

        let selections = self
            .selection
            .iter()
            .map(|region| self.data.slice_to_cow(region.min()..=region.max()).to_vec())
            .collect();
        self.registers.insert(reg, selections);
    }

    pub fn overflow_sel_style(&self) -> Option<OverflowSelectionStyle> {
        let last_sel = self.selection.iter().last().unwrap();
        let len = self.data.len();
        if last_sel.caret == len && last_sel.tail == len {
            Some(OverflowSelectionStyle::CursorTail)
        } else if last_sel.caret == len {
            Some(OverflowSelectionStyle::Cursor)
        } else if last_sel.tail == len {
            Some(OverflowSelectionStyle::Tail)
        } else {
            None
        }
    }

    pub fn update_path_if_missing(&mut self, path: impl Into<PathBuf>) {
        if self.path.is_none() {
            self.path = Some(path.into());
        }
    }
}

pub struct BuffrCollection {
    list: Vec<CurrentBuffer>,
    cur_buf_index: usize,
}

impl Default for BuffrCollection {
    fn default() -> Self {
        Self::new()
    }
}

// in current_buffers.rs
impl BuffrCollection {

    /*
    Note: impl BuffrCollection does NOT need a load_next_chunk function at all.
    */
    
    pub fn new() -> BuffrCollection {
        BuffrCollection::with_current_buffer(CurrentBuffer::default())
    }

    pub fn with_current_buffer(buf: CurrentBuffer) -> BuffrCollection {
        BuffrCollection {
            cur_buf_index: 0,
            list: vec![buf],
        }
    }

    pub fn current(&self) -> &CurrentBuffer {
        &self.list[self.cur_buf_index]
    }
    pub fn current_mut(&mut self) -> &mut CurrentBuffer {
        &mut self.list[self.cur_buf_index]
    }

    pub fn iter(&self) -> impl Iterator<Item = &CurrentBuffer> {
        self.list.iter()
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut CurrentBuffer> {
        self.list.iter_mut()
    }

    pub fn switch_current_buffer(&mut self, filename: impl AsRef<Path>) -> Result<(), std::io::Error> {
        let canon = filename.as_ref().canonicalize()?;
        for (i, buf) in self.list.iter().enumerate() {
            if let Some(path) = &buf.path {
                if path.canonicalize()? == canon {
                    self.cur_buf_index = i;
                    return Ok(());
                }
            }
        }

        self.list.push(CurrentBuffer::from_data_and_path(
            std::fs::read(&filename)?,
            Some(filename.as_ref().to_owned()),
        ));
        self.cur_buf_index = self.list.len() - 1;
        Ok(())
    }

    pub fn delete_current(&mut self) {
        self.list.remove(self.cur_buf_index);
        self.cur_buf_index = self.cur_buf_index.saturating_sub(1);
        if self.list.is_empty() {
            self.list.push(CurrentBuffer::default());
        }
    }
}
