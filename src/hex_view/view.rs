use std::cell::Cell;
use std::cmp;
use std::collections::BTreeSet;
use std::fmt;
use std::io::{
    SeekFrom,
    Seek,
    Write,
    Read,
    Error,
    ErrorKind,
};
use std::ops::Range;
use std::time;
use std::fs::File;
use std::fs::OpenOptions;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue, style,
    style::{Color, Stylize},
    terminal, 
    QueueableCommand, 
    Result,
};
use xi_rope::{
    Interval,
    Delta,
    Rope
};
use xi_rope::tree::TreeBuilder;
use xi_rope::multiset::SubsetBuilder;
use crate::byte_rope::Bytes;  // TODO Horrible name that will collide this must be changed
use std::time::{
    SystemTime, 
    UNIX_EPOCH
};
use super::byte_properties::BytePropertiesFormatter;
use super::{make_padding, PrioritizedStyle, Priority, StylingCommand};
use crate::current_buffer::*;
use crate::hex_view::OutputColorizer;
use crate::modes;
use crate::modes::mode::{DirtyBytes, Mode, ModeTransition};
use crate::selection::Direction;
// use std::path::Path;
use std::env;

use crate::byte_rope::Rope as CustomByteRope;

const VERTICAL: &str = "│";
const LEFTARROW: &str = "";

// Oh my Uma, it's a Debug-Log... 
// Why is this returning a result???
// todo THis never does ANYTHING
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


struct MixedRepr(u8);

impl fmt::Display for MixedRepr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.0.is_ascii_graphic() || self.0 == 0x20 {
            write!(f, "{}", char::from(self.0))
        } else {
            write!(f, "<{:02x}>", self.0)
        }
    }
}

trait StatusLinePrompter: Mode {
    fn render_with_size(
        &self,
        stdout: &mut dyn Write,
        max_width: usize,
        last_start_col: usize,
    ) -> Result<usize>;
}

macro_rules! d_queue {
    ($writer:expr $(, $command:expr)* $(,)?) => {{
        Ok::<_, crossterm::ErrorKind>(&mut *$writer)
            $(.and_then(|mut writer| {
                QueueableCommand::queue(&mut writer, $command)?;
                Ok(writer)
            }))*
            .map(|_| ())
    }}
}

impl StatusLinePrompter for modes::search::Search {
    fn render_with_size(
        &self,
        stdout: &mut dyn Write,
        mut max_width: usize,
        last_start_col: usize,
    ) -> Result<usize> {
        let mut start_column = last_start_col;
        d_queue!(
            stdout,
            style::PrintStyledContent(
                style::style("search:")
                    .with(style::Color::White)
                    .on(style::Color::Blue),
            )
        )?;
        max_width -= "search:".len();

        // Make sure start_column is between self.cursor and the length of the pattern
        if self.pattern.pieces.len() <= start_column {
            start_column = std::cmp::max(1, self.pattern.pieces.len()) - 1;
        } else if self.cursor < start_column {
            start_column = self.cursor;
        }

        if self.hex {
            if self.cursor >= start_column + max_width / 3 {
                start_column = self.cursor - max_width / 3 + 1;
            }
            let last_byte = std::cmp::min(self.pattern.pieces.len(), start_column + max_width / 3);

            let normalized_cursor = self.cursor - start_column;
            for (i, piece) in self.pattern.pieces[start_column..last_byte]
                .iter()
                .enumerate()
            {
                match piece {
                    PatternPiece::Literal(byte) if normalized_cursor != i => {
                        d_queue!(stdout, style::Print(format!("{:02x} ", byte)))?
                    }
                    PatternPiece::Literal(byte)
                        if normalized_cursor == i && self.hex_half.is_some() =>
                    {
                        d_queue!(
                            stdout,
                            style::Print(format!("{:x}", byte >> 4)),
                            style::PrintStyledContent(
                                style::style(format!("{:x}", byte & 0xf))
                                    .with(style::Color::Black)
                                    .on(style::Color::White)
                            ),
                            style::Print(" "),
                        )?
                    }
                    PatternPiece::Literal(byte) => d_queue!(
                        stdout,
                        style::PrintStyledContent(
                            style::style(format!("{:02x}", byte))
                                .with(style::Color::Black)
                                .on(style::Color::White)
                        ),
                        style::Print(" "),
                    )?,
                    PatternPiece::Wildcard if normalized_cursor != i => d_queue!(
                        stdout,
                        style::PrintStyledContent(style::style("** ").with(style::Color::DarkRed))
                    )?,
                    PatternPiece::Wildcard => d_queue!(
                        stdout,
                        style::PrintStyledContent(
                            style::style("**")
                                .with(style::Color::DarkRed)
                                .on(style::Color::White)
                        ),
                        style::Print(" "),
                    )?,
                }
            }
            if self.cursor == self.pattern.pieces.len() {
                d_queue!(
                    stdout,
                    style::PrintStyledContent(
                        style::style("  ")
                            .with(style::Color::Black)
                            .on(style::Color::White)
                    ),
                    style::Print(" "),
                )?
            }

            return Ok(start_column);
        }

        max_width -= (self.cursor == self.pattern.pieces.len()) as usize;

        use modes::search::PatternPiece;
        let mut lengths = self.pattern.pieces[start_column..]
            .iter()
            .map(|x| match x {
                PatternPiece::Wildcard => 1,
                PatternPiece::Literal(0x20) => 1,
                PatternPiece::Literal(byte) if byte.is_ascii_graphic() => 1,
                PatternPiece::Literal(_) => 4,
            })
            .collect::<Vec<_>>();
        let required_length: usize = lengths[..self.cursor - start_column].iter().sum();
        if required_length > max_width {
            let mut remaining_delta = (required_length - max_width) as isize;
            let num_dropped_pieces = lengths
                .iter()
                .position(|&x| {
                    let is_done = remaining_delta <= 0;
                    remaining_delta -= x as isize;
                    is_done
                })
                .unwrap();
            start_column += num_dropped_pieces;
            lengths.drain(..num_dropped_pieces);
        }

        let normalized_cursor = self.cursor - start_column;
        for ((i, piece), length) in self.pattern.pieces[start_column..]
            .iter()
            .enumerate()
            .zip(lengths)
        {
            if max_width < length {
                break;
            }
            max_width -= length;
            match piece {
                PatternPiece::Literal(byte)
                    if normalized_cursor != i && (byte.is_ascii_graphic() || *byte == 0x20) =>
                {
                    d_queue!(stdout, style::Print(format!("{}", *byte as char)))?
                }
                PatternPiece::Literal(byte) if normalized_cursor != i => d_queue!(
                    stdout,
                    style::PrintStyledContent(
                        style::style(format!("<{:02x}>", byte))
                            .with(style::Color::Black)
                            .on(style::Color::DarkGrey)
                    ),
                )?,
                PatternPiece::Literal(byte)
                    if normalized_cursor == i && (byte.is_ascii_graphic() || *byte == 0x20) =>
                {
                    d_queue!(
                        stdout,
                        style::PrintStyledContent(
                            style::style(format!("{}", *byte as char))
                                .with(style::Color::Black)
                                .on(style::Color::White)
                        ),
                    )?
                }
                PatternPiece::Literal(byte) => d_queue!(
                    stdout,
                    style::PrintStyledContent(
                        style::style(format!("<{:02x}>", byte))
                            .with(style::Color::Black)
                            .on(style::Color::White)
                    ),
                )?,
                PatternPiece::Wildcard if normalized_cursor != i => d_queue!(
                    stdout,
                    style::PrintStyledContent(style::style("*").with(style::Color::DarkRed))
                )?,
                PatternPiece::Wildcard => d_queue!(
                    stdout,
                    style::PrintStyledContent(
                        style::style("*")
                            .with(style::Color::DarkRed)
                            .on(style::Color::White)
                    ),
                )?,
            }
        }

        if self.cursor == self.pattern.pieces.len() {
            d_queue!(
                stdout,
                style::PrintStyledContent(
                    style::style(" ")
                        .with(style::Color::Black)
                        .on(style::Color::White)
                ),
            )?;
        }

        Ok(start_column)
    }
}

impl StatusLinePrompter for modes::command::Command {
    fn render_with_size(
        &self,
        stdout: &mut dyn Write,
        mut max_width: usize,
        last_start_col: usize,
    ) -> Result<usize> {
        let mut start_column = last_start_col;
        d_queue!(
            stdout,
            style::PrintStyledContent(
                style::style(":")
                    .with(style::Color::White)
                    .on(style::Color::Blue),
            )
        )?;
        max_width -= 1;

        // Make sure start_column is between self.cursor and the length of the pattern
        if self.command.len() <= start_column {
            start_column = std::cmp::max(1, self.command.len()) - 1;
        } else if self.cursor < start_column {
            start_column = self.cursor;
        }

        max_width -= (self.cursor == self.command.len()) as usize;

        let required_length = self.cursor - start_column;
        if required_length > max_width {
            start_column += required_length - max_width;
        }

        d_queue!(
            stdout,
            style::Print(
                &self.command
                    [start_column..std::cmp::min(self.command.len(), start_column + max_width)]
            )
        )?;

        if self.cursor == self.command.len() {
            d_queue!(
                stdout,
                style::PrintStyledContent(
                    style::style(" ")
                        .with(style::Color::Black)
                        .on(style::Color::White)
                ),
            )?;
        }

        Ok(start_column)
    }
}

pub struct HexView {
    buffr_collection: BuffrCollection,
    size: (u16, u16),
    bytes_per_line: usize,
    start_offset: usize,
    last_visible_rows: Cell<usize>,
    last_visible_prompt_col: Cell<usize>,
    last_draw_time: time::Duration,
    colorizer: OutputColorizer,

    mode: Box<dyn Mode>,
    info: Option<String>,
}

impl HexView {
    /// Checks if the buffer size exceeds the threshold for trimming
    /// Uses dynamic chunk size based on terminal height
    fn should_trim_buffer(&self) -> bool {
        let (_, height) = terminal::size().unwrap_or((80, 23));
        let chunk_size = (height as usize - 1) * 16;
        let buffer_threshold = chunk_size * 3;  // 3x chunk size threshold
        
        let current_size = self.buffr_collection.current().data.len();
        debug_log(&format!("Buffer check: size={}, threshold={}", 
            current_size, buffer_threshold));
        
        current_size > buffer_threshold
    }
    
    fn is_near_bottom(&self) -> bool {
        debug_log("is_near_bottom()");
        
        let current_buffer = self.buffr_collection.current();
        let visible_rows = self.size.1 as usize - 2;  // Subtract status bar
        let bytes_per_line = self.bytes_per_line;
        
        let total_buffer_bytes = current_buffer.data.len();
        let current_view_end = self.start_offset + (visible_rows * bytes_per_line);
        
        // Within last 10% of buffer
        total_buffer_bytes - current_view_end < (total_buffer_bytes / 10)
    }
    
    fn is_near_top(&self) -> bool {
        // Within first 10% of buffer
        self.start_offset < (self.buffr_collection.current().data.len() / 10)
    }

    // Similar for add_chunk_to_bottom:
    fn add_chunk_to_bottom(&mut self, chunk_size: usize) -> std::result::Result<(), std::io::Error> {
        debug_log(&format!("Attempting to add chunk to bottom, size={}", chunk_size));
        
        let current_buffer = self.buffr_collection.current();
        if let Some(path) = &current_buffer.path {
            let mut file = File::open(path)?;
            let current_data_len = current_buffer.data.len();
            
            debug_log(&format!("Current buffer size: {}", current_data_len));
            
            file.seek(SeekFrom::Start(current_data_len as u64))?;
            
            let mut next_chunk = vec![0; chunk_size];
            let bytes_read = file.read(&mut next_chunk)?;
            
            debug_log(&format!("Bytes read from file: {}", bytes_read));
            
            if bytes_read > 0 {
                // Create new rope using TreeBuilder
                let mut builder = TreeBuilder::new();
                builder.push_leaf(Bytes(next_chunk[..bytes_read].to_vec()));
                let chunk_node = builder.build();
                
                // Create delta
                let delta = Delta::simple_edit(
                    Interval::new(current_data_len, current_data_len), 
                    chunk_node,
                    current_buffer.data.len()
                );
                
                let old_len = current_buffer.data.len();
                let mut current_data = current_buffer.data.clone();
                current_data = current_data.apply_delta(&delta);
                let new_len = current_data.len();
                
                debug_log(&format!("Buffer size changed after append: {} -> {}", old_len, new_len));
                
                self.buffr_collection.current_mut().data = current_data;
            }
        }
        Ok(())
    }    

    fn add_chunk_to_top(&mut self, chunk_size: usize) -> std::result::Result<(), std::io::Error> {
        debug_log(&format!("add_chunk_to_top, size={:?}", chunk_size));
        
        let current_buffer = self.buffr_collection.current();
        if let Some(path) = &current_buffer.path {
            let mut file = File::open(path)?;
            
            // Calculate how much to go back
            let start_pos = self.start_offset.saturating_sub(chunk_size);
            file.seek(SeekFrom::Start(start_pos as u64))?;
            
            let mut prev_chunk = vec![0; chunk_size];
            let bytes_read = file.read(&mut prev_chunk)?;
            
            if bytes_read > 0 {
                // Create new rope using TreeBuilder (same pattern as add_chunk_to_bottom)
                let mut builder = TreeBuilder::new();
                builder.push_leaf(Bytes(prev_chunk[..bytes_read].to_vec()));
                let chunk_node = builder.build();
                
                // Create delta for insertion at the beginning
                let delta = Delta::simple_edit(
                    Interval::new(0, 0), 
                    chunk_node,
                    current_buffer.data.len()
                );
                
                // Apply delta
                let mut current_data = current_buffer.data.clone();
                current_data = current_data.apply_delta(&delta);
                self.buffr_collection.current_mut().data = current_data;
                
                // Adjust start_offset
                self.start_offset = start_pos;
            }
        }
        Ok(())
    }

    /// # Trim Buffer Bottom Function
    /// 
    /// This function trims the bottom portion of a buffer that uses a custom Rope implementation
    /// for handling binary data. The function is part of a windowing/pagination system for large files.
    /// 
    /// ## Technical Details
    /// - Uses a custom Rope implementation (CustomByteRope) specifically designed for binary data
    /// - Maintains a fixed-size window of data in memory
    /// - Implements trimming through xi-rope's TreeBuilder system
    /// 
    /// ## Data Structure Details
    /// The function works with:
    /// - CustomByteRope: A specialized rope implementation for binary data
    /// - TreeBuilder: Xi-rope's builder for creating rope structures
    /// - Bytes: A wrapper struct for Vec<u8> that implements the Leaf trait
    /// 
    /// ## Parameters
    /// * `chunk_size`: The number of bytes to trim from the bottom of the buffer
    /// 
    /// ## Implementation Notes
    /// 1. Safety Checks:
    ///    - Prevents trimming if chunk_size is 0
    ///    - Prevents trimming if buffer is too small (less than 2x chunk_size)
    ///    - Maintains minimum buffer size requirements
    /// 
    /// 2. Rope Construction Process:
    ///    - Creates a TreeBuilder instance
    ///    - Collects chunks from the existing rope using iter_chunks
    ///    - Wraps the collected bytes in the Bytes struct
    ///    - Builds a new rope with the remaining data
    /// 
    /// 3. Buffer Management:
    ///    - Updates start_offset to reflect the trimmed portion
    ///    - Maintains proper buffer boundaries
    ///    - Handles edge cases for small buffers
    /// 
    /// ## Common Issues and Solutions
    /// - Empty Segment Error: Prevented by checking chunks.is_empty()
    /// - Type Mismatches: Resolved by using CustomByteRope wrapper
    /// - Memory Management: Handled through proper chunk size calculations
    /// 
    /// ## Example Buffer Flow:
    /// ```text
    /// Initial Buffer: [A B C D E F G H]  (size 8)
    /// chunk_size: 3
    /// After Trim:   [A B C D E]         (size 5)
    /// ```
    fn trim_buffer_bottom(&mut self, chunk_size: usize) {
        debug_log(&format!("trim_buffer_bottom, size={:?}", chunk_size));
        
        let current_buffer = self.buffr_collection.current_mut();
        let total_len = current_buffer.data.len();
        
        // Safety checks
        if chunk_size == 0 || total_len <= chunk_size * 2 {
            debug_log("Buffer too small for trimming");
            return;
        }

        let keep_len = total_len - chunk_size;
        if keep_len > 0 {
            debug_log(&format!(
                "Trimming bottom: start_offset={}, total_len={}, keeping {} bytes", 
                self.start_offset, total_len, keep_len
            ));
            
            // Create new rope using your custom implementation
            let mut builder = TreeBuilder::new();
            let chunks: Vec<u8> = current_buffer.data
                .iter_chunks(0..keep_len)
                .flat_map(|chunk| chunk.to_vec())
                .collect();
                
            if !chunks.is_empty() {
                builder.push_leaf(Bytes(chunks));
            }
            
            let old_len = current_buffer.data.len();
            current_buffer.data = CustomByteRope(builder.build());
            let new_len = current_buffer.data.len();
            
            debug_log(&format!("Buffer size changed: {} -> {}", old_len, new_len));
            
            self.start_offset += chunk_size;
        }
    }

    /// # Trim Buffer Top Function
    /// 
    /// This function trims the top portion of a buffer using a custom Rope implementation.
    /// It's essential for maintaining a sliding window view of large files in memory.
    /// 
    /// ## Technical Details
    /// - Implements top-down trimming for a custom binary Rope structure
    /// - Part of a larger pagination/windowing system
    /// - Uses xi-rope's underlying tree structure
    /// 
    /// ## Data Structure Relationships
    /// ```text
    /// CustomByteRope
    ///   └─ Node<RopeInfo>
    ///       └─ Bytes(Vec<u8>)
    /// ```
    /// 
    /// ## Parameters
    /// * `chunk_size`: The number of bytes to remove from the top of the buffer
    /// 
    /// ## Implementation Notes
    /// 1. Buffer Management:
    ///    - Maintains buffer size constraints
    ///    - Updates start_offset for proper position tracking
    ///    - Handles boundary conditions
    /// 
    /// 2. Data Processing Flow:
    ///    a. Validates input and buffer size
    ///    b. Extracts required portion using iter_chunks
    ///    c. Rebuilds rope structure with TreeBuilder
    ///    d. Updates buffer state
    /// 
    /// 3. Safety Considerations:
    ///    - Prevents buffer underflow
    ///    - Maintains minimum size requirements
    ///    - Handles edge cases for small files
    /// 
    /// ## Error Prevention
    /// - Checks for minimum buffer size (2 * chunk_size)
    /// - Validates chunk_size > 0
    /// - Ensures proper rope structure maintenance
    /// 
    /// ## Memory Management
    /// - Creates new rope structure efficiently
    /// - Properly handles data transfer between old and new ropes
    /// - Maintains leaf size constraints
    /// 
    /// ## Example Operation:
    /// ```text
    /// Before: [A B C D E F G H]  (offset 0)
    /// Trim 3: [D E F G H]        (offset 3)
    /// ```
    /// 
    /// ## Related Components
    /// - Buffer Interface
    /// - File Loading System
    /// - Pagination Controller
    /// 
    /// ## Common Debugging Points
    /// 1. Offset Calculations
    /// 2. Rope Structure Integrity
    /// 3. Memory Usage Patterns
    /// 4. Edge Case Handling
    fn trim_buffer_top(&mut self, chunk_size: usize) {
        debug_log("\n=== Trim Buffer Top ===");
        debug_log(&format!("trim_buffer_top, size={:?}", chunk_size));
        
        let current_buffer = self.buffr_collection.current_mut();
        let total_len = current_buffer.data.len();
        
        // Safety checks
        if chunk_size == 0 || total_len <= chunk_size * 2 {
            debug_log(&format!("Cannot trim: chunk_size={}, total_len={}", chunk_size, total_len));
            return;
        }

        if chunk_size < total_len {
            debug_log(&format!("Trimming first {} bytes from buffer of size {}", chunk_size, total_len));
            
            // Create new rope using your custom implementation
            let mut builder = TreeBuilder::new();
            let chunks: Vec<u8> = current_buffer.data
                .iter_chunks(chunk_size..total_len)
                .flat_map(|chunk| chunk.to_vec())
                .collect();
                
            if !chunks.is_empty() {
                builder.push_leaf(Bytes(chunks));
            }
            
            let old_len = current_buffer.data.len();
            current_buffer.data = CustomByteRope(builder.build());
            let new_len = current_buffer.data.len();
            
            debug_log(&format!("Buffer trimmed: {} -> {}", old_len, new_len));
            
            self.start_offset = self.start_offset.saturating_sub(chunk_size);
        }
    }

    fn manage_buffer(&mut self) -> std::result::Result<(), std::io::Error> {

        let chunk_size = 368;  // Your previous chunk size
        
        debug_log(&format!("manage_buffer, size={:?}", chunk_size));
        
        if self.is_near_bottom() {
            self.add_chunk_to_bottom(chunk_size)?;
            self.trim_buffer_top(chunk_size);
        }
        
        if self.is_near_top() {
            self.add_chunk_to_top(chunk_size)?;
            self.trim_buffer_bottom(chunk_size);
        }
        
        Ok(())
    }
        
    pub fn with_buffr_collection(buffr_collection: BuffrCollection) -> HexView {
        HexView {
            buffr_collection,
            bytes_per_line: 0x10,
            start_offset: 0,
            size: terminal::size().unwrap(),
            last_visible_rows: Cell::new(0),
            last_visible_prompt_col: Cell::new(0),
            last_draw_time: Default::default(),
            colorizer: OutputColorizer::new(),

            mode: Box::new(modes::normal::Normal::new()),
            info: None,
        }
    }

    pub fn set_bytes_per_line(&mut self, bpl: usize) {
        self.bytes_per_line = bpl;
    }

    fn draw_hex_row(
        &self,
        stdout: &mut impl Write,
        styled_bytes: impl IntoIterator<Item = (u8, StylingCommand)>,
    ) -> Result<()> {
        for (byte, style_cmd) in styled_bytes.into_iter() {
            self.colorizer.draw_hex_byte(stdout, byte, &style_cmd)?;
        }
        Ok(())
    }

    fn draw_ascii_row(
        &self,
        stdout: &mut impl Write,
        styled_bytes: impl IntoIterator<Item = (u8, StylingCommand)>,
    ) -> Result<()> {
        for (byte, style_cmd) in styled_bytes.into_iter() {
            self.colorizer.draw_ascii_byte(stdout, byte, &style_cmd)?;
        }
        Ok(())
    }

    fn draw_separator(&self, stdout: &mut impl Write) -> Result<()> {
        queue!(stdout, style::SetForegroundColor(Color::White))?;
        queue!(stdout, style::Print(format!("{} ", VERTICAL)))
    }
    
    /// Safely calculates if an offset is within valid bounds
    fn is_valid_offset(&self, offset: usize) -> bool {
        let buffer_size = self.buffr_collection.current().data.len();
        offset < buffer_size
    }
    
    /// Converts a byte offset in the file to a screen row number (0-based).
    /// 
    /// # Details
    /// - Takes a file byte offset (absolute position in file)
    /// - Subtracts start_offset to get relative position in current view
    /// - Divides by bytes_per_line (usually 16) to get row number
    /// - Checks if resulting row would fit on screen
    /// 
    /// # Arguments
    /// * `offset` - Absolute byte offset in file (e.g., 0 = first byte, 16 = start of second line)
    /// 
    /// # Returns
    /// * `Ok(u16)` - Screen row number (0 = top of screen)
    /// * `Err` - If calculated row would be outside visible screen area
    /// 
    /// # Example
    /// ```
    /// // If start_offset = 32 (viewing starts at 3rd line of file)
    /// // bytes_per_line = 16
    /// // screen height = 24
    /// 
    /// offset_to_row(48)  // -> Ok(1)  // (48-32)/16 = 1 (second line on screen)
    /// offset_to_row(32)  // -> Ok(0)  // (32-32)/16 = 0 (first line on screen)
    /// offset_to_row(416) // -> Err    // (416-32)/16 = 24 (beyond screen height)
    /// ```
    /// 
    /// # Technical Notes
    /// - Screen coordinates are 0-based
    /// - Assumes fixed-width display (16 bytes per line)
    /// - Must account for start_offset (current scroll position)
    /// - Must fit within terminal height (self.size.1)
    /// 
    /// # Error Conditions
    /// - Returns Err if calculated row >= screen height
    /// - Can handle offsets smaller than start_offset (negative rows filtered)
    /// 
    fn offset_to_row(&self, offset: usize) -> Result<u16> {
        debug_log(&format!("offset_to_row: offset={}, start_offset={}", 
            offset, self.start_offset));
    
        // Check for underflow condition
        if offset < self.start_offset {
            debug_log(&format!("offset_to_row: offset {} is before start_offset {}", 
                offset, self.start_offset));
            return Err(Error::new(ErrorKind::Other, "Offset before visible area"));
        }
    
        let row = (offset - self.start_offset) / self.bytes_per_line;
        if row >= self.size.1 as usize {
            debug_log(&format!("offset_to_row: row {} exceeds screen height {}", 
                row, self.size.1));
            return Err(Error::new(ErrorKind::Other, "Row outside visible area"));
        }
        Ok(row as u16)
    }

    // in view.rs
    // in impl HexView {}
    fn draw_row(
        &self,
        stdout: &mut impl Write,
        bytes: &[u8],
        offset: usize,
        mark_commands: &[StylingCommand],
        end_style: Option<StylingCommand>,
        byte_properties: &mut BytePropertiesFormatter,
    ) -> Result<()> {
        debug_log(&format!("draw_row, offset={:?}", offset));
        // let row_num = self.offset_to_row(offset).unwrap(); // panic here (don't ever use unwrap!!)
        
        // Replace unwrap with proper error handling
        let row_num = match self.offset_to_row(offset) {
            Ok(row) => row,
            Err(e) => {
                debug_log(&format!("Could not convert offset {} to row: {}", offset, e));
                return Ok(());  // Skip drawing this row instead of panicking
            }
        };

        queue!(stdout, cursor::MoveTo(0, row_num))?;
        queue!(
            stdout,
            style::Print(" ".to_string()), // Padding
        )?;
        self.draw_hex_row(
            stdout,
            bytes.iter().copied().zip(mark_commands.iter().cloned()),
        )?;

        let mut padding_length = if bytes.is_empty() {
            self.bytes_per_line * 3
        } else {
            (self.bytes_per_line - bytes.len()) % self.bytes_per_line * 3
        };

        if let Some(style_cmd) = &end_style {
            padding_length -= 2;

            self.colorizer
                .draw(stdout, ' ', &style_cmd.clone().with_mid_to_end())?;
            self.colorizer
                .draw(stdout, ' ', &style_cmd.clone().take_end_only())?;
        }

        queue!(stdout, style::Print(make_padding(padding_length)))?;
        self.draw_separator(stdout)?;

        self.draw_ascii_row(
            stdout,
            bytes.iter().copied().zip(mark_commands.iter().cloned()),
        )?;

        let mut padding_length = if bytes.is_empty() {
            self.bytes_per_line
        } else {
            (self.bytes_per_line - bytes.len()) % self.bytes_per_line
        } + 1;

        if let Some(style_cmd) = end_style {
            padding_length -= 1;
            self.colorizer
                .draw(stdout, ' ', &style_cmd.take_end_only())?;
        }

        queue!(stdout, style::Print(make_padding(padding_length)))?;
        self.draw_separator(stdout)?;

        byte_properties.draw_line(stdout, &self.colorizer)?;

        queue!(stdout, terminal::Clear(terminal::ClearType::UntilNewLine))?;

        Ok(())
    }

    fn visible_bytes(&self) -> Range<usize> {
        self.start_offset
            ..cmp::min(
                self.buffr_collection.current().data.len() + 1,
                self.start_offset + (self.size.1 - 1) as usize * self.bytes_per_line,
            )
    }

    fn default_style(&self) -> PrioritizedStyle {
        PrioritizedStyle {
            style: style::ContentStyle::new()
                .with(style::Color::White)
                .on(style::Color::Reset),
            priority: Priority::Basic,
        }
    }

    fn active_selection_style(&self) -> PrioritizedStyle {
        PrioritizedStyle {
            style: style::ContentStyle::new()
                .with(style::Color::Black)
                .on(style::Color::Rgb {
                    r: 110,
                    g: 97,
                    b: 16,
                }),
            priority: Priority::Selection,
        }
    }

    fn inactive_selection_style(&self) -> PrioritizedStyle {
        PrioritizedStyle {
            style: style::ContentStyle::new()
                .with(style::Color::Black)
                .on(style::Color::DarkGrey),
            priority: Priority::Selection,
        }
    }

    fn active_caret_style(&self) -> PrioritizedStyle {
        PrioritizedStyle {
            style: style::ContentStyle::new()
                .with(style::Color::AnsiValue(16))
                .on(style::Color::Rgb {
                    r: 107,
                    g: 108,
                    b: 128,
                }),
            priority: Priority::Cursor,
        }
    }

    fn inactive_caret_style(&self) -> PrioritizedStyle {
        PrioritizedStyle {
            style: style::ContentStyle::new()
                .with(style::Color::Black)
                .on(style::Color::DarkGrey),
            priority: Priority::Cursor,
        }
    }

    fn empty_caret_style(&self) -> PrioritizedStyle {
        PrioritizedStyle {
            style: style::ContentStyle::new().on(style::Color::Green),
            priority: Priority::Cursor,
        }
    }

    fn mark_commands(&self, visible: Range<usize>) -> Vec<StylingCommand> {
        let mut mark_commands = vec![StylingCommand::default(); visible.len()];
        let mut selected_regions = self
            .buffr_collection
            .current()
            .selection
            .regions_in_range(visible.start, visible.end);
        let mut command_stack = vec![self.default_style()];
        let start = visible.start;

        // Add to command stack those commands that being out of bounds
        if !selected_regions.is_empty() && selected_regions[0].min() < start {
            command_stack.push(if selected_regions[0].is_main() {
                self.active_selection_style()
            } else {
                self.inactive_selection_style()
            });
        }

        for i in visible {
            let normalized = i - start;
            if !selected_regions.is_empty() {
                if selected_regions[0].min() == i {
                    command_stack.push(if selected_regions[0].is_main() {
                        self.active_selection_style()
                    } else {
                        self.inactive_selection_style()
                    });
                    mark_commands[normalized] = mark_commands[normalized]
                        .clone()
                        .with_start_style(command_stack.last().unwrap().clone());
                }
                if selected_regions[0].caret == i {
                    let base_style = command_stack.last().unwrap().clone();
                    let mut caret_cmd = mark_commands[normalized].clone();
                    let caret_style = if selected_regions[0].is_main() {
                        self.active_caret_style()
                    } else {
                        self.inactive_caret_style()
                    };
                    if self.mode.has_half_cursor() {
                        if i == selected_regions[0].min() {
                            caret_cmd = caret_cmd
                                .with_mid_style(caret_style)
                                .with_end_style(base_style);
                        } else {
                            caret_cmd = caret_cmd
                                .with_start_style(base_style)
                                .with_mid_style(caret_style);
                        }
                    } else {
                        caret_cmd = caret_cmd
                            .with_start_style(caret_style)
                            .with_end_style(base_style);
                    }
                    mark_commands[normalized] = caret_cmd;
                }
                if selected_regions[0].max() == i {
                    mark_commands[normalized] = mark_commands[normalized]
                        .clone()
                        .with_end_style(command_stack[command_stack.len() - 2].clone());
                }
            }

            if i % self.bytes_per_line == 0 && mark_commands[normalized].start_style().is_none() {
                // line starts: restore applied style
                mark_commands[normalized] = mark_commands[normalized]
                    .clone()
                    .with_start_style(command_stack.last().unwrap().clone());
            } else if (i + 1) % self.bytes_per_line == 0 {
                // line ends: apply default style
                mark_commands[normalized] = mark_commands[normalized]
                    .clone()
                    .with_end_style(self.default_style());
            }

            if !selected_regions.is_empty() && selected_regions[0].max() == i {
                // Must be popped after line config
                command_stack.pop();
                selected_regions = &selected_regions[1..];
            }
        }

        mark_commands
    }

    fn calculate_powerline_length(&self) -> usize {
        let buf = self.buffr_collection.current();
        let mut length = 0;
        length += 1; // leftarrow
        length += 2 + buf.name().len();
        if buf.dirty {
            length += 3;
        }
        length += 1; // leftarrow
        length += 2 + self.mode.name().len();
        length += 1; // leftarrow
        length += format!(
            " {} sels ({}) ",
            buf.selection.len(),
            buf.selection.main_selection + 1
        )
        .len();
        length += 1; // leftarrow
        if !buf.data.is_empty() {
            length += format!(
                " {:x}/{:x} ",
                buf.selection.main_cursor_offset(),
                buf.data.len() - 1
            )
            .len();
        } else {
            length += " empty ".len();
        }
        length
    }

    fn draw_statusline_here(&self, stdout: &mut impl Write) -> Result<()> {
        let buf = self.buffr_collection.current();
        queue!(
            stdout,
            style::PrintStyledContent(style::style(LEFTARROW).with(Color::Red)),
            style::PrintStyledContent(
                style::style(format!(
                    " {}{} ",
                    self.buffr_collection.current().name(),
                    if self.buffr_collection.current().dirty {
                        "[+]"
                    } else {
                        ""
                    }
                ))
                .with(Color::White)
                .on(Color::Red)
            ),
            style::PrintStyledContent(
                style::style(LEFTARROW)
                    .with(Color::DarkYellow)
                    .on(Color::Red)
            ),
            style::PrintStyledContent(
                style::style(format!(" {} ", self.mode.name()))
                    .with(Color::AnsiValue(16))
                    .on(Color::DarkYellow)
            ),
            style::PrintStyledContent(
                style::style(LEFTARROW)
                    .with(Color::White)
                    .on(Color::DarkYellow)
            ),
            style::PrintStyledContent(
                style::style(format!(
                    " {} sels ({}) ",
                    buf.selection.len(),
                    buf.selection.main_selection + 1
                ))
                .with(Color::AnsiValue(16))
                .on(Color::White)
            ),
        )?;
        if !buf.data.is_empty() {
            queue!(
                stdout,
                style::PrintStyledContent(
                    style::style(LEFTARROW).with(Color::Blue).on(Color::White)
                ),
                style::PrintStyledContent(
                    style::style(format!(
                        " {:x}/{:x} ",
                        buf.selection.main_cursor_offset(),
                        buf.data.len() - 1,
                    ))
                    .with(Color::White)
                    .on(Color::Blue),
                ),
            )?;
        } else {
            queue!(
                stdout,
                style::PrintStyledContent(
                    style::style(LEFTARROW).with(Color::Blue).on(Color::White)
                ),
                style::PrintStyledContent(
                    style::style(" empty ").with(Color::White).on(Color::Blue),
                ),
            )?;
        }
        Ok(())
    }

    fn draw_statusline(&self, stdout: &mut impl Write) -> Result<()> {
        let line_length = self.calculate_powerline_length();
        if let Some(info) = &self.info {
            queue!(
                stdout,
                cursor::MoveTo(0, self.size.1 - 1),
                terminal::Clear(terminal::ClearType::CurrentLine),
                style::PrintStyledContent(
                    style::style(info)
                        .with(style::Color::White)
                        .on(style::Color::Blue)
                ),
                cursor::MoveTo(self.size.0 - line_length as u16, self.size.1),
            )?;
        } else {
            queue!(
                stdout,
                cursor::MoveTo(self.size.0 - line_length as u16, self.size.1),
                terminal::Clear(terminal::ClearType::CurrentLine),
            )?;
        }

        self.draw_statusline_here(stdout)?;

        let any_mode = self.mode.as_any();
        let prompter = if let Some(statusliner) = any_mode.downcast_ref::<modes::search::Search>() {
            Some(statusliner as &dyn StatusLinePrompter)
        } else {
            any_mode
                .downcast_ref::<modes::command::Command>()
                .map(|statusliner| statusliner as &dyn StatusLinePrompter)
        };

        if let Some(statusliner) = prompter {
            queue!(stdout, cursor::MoveTo(0, self.size.1))?;
            let prev_col = self.last_visible_prompt_col.get();
            let new_col = statusliner.render_with_size(stdout, self.size.0 as usize, prev_col)?;
            self.last_visible_prompt_col.set(new_col);
        }

        Ok(())
    }

    fn overflow_cursor_style(&self) -> Option<StylingCommand> {
        self.buffr_collection.current().overflow_sel_style().map(|style| {
            match style {
                OverflowSelectionStyle::CursorTail | OverflowSelectionStyle::Cursor
                    if self.mode.has_half_cursor() =>
                {
                    StylingCommand::default().with_mid_style(self.empty_caret_style())
                }
                OverflowSelectionStyle::CursorTail | OverflowSelectionStyle::Cursor => {
                    StylingCommand::default().with_start_style(self.empty_caret_style())
                }
                OverflowSelectionStyle::Tail => StylingCommand::default(),
            }
            .with_end_style(self.default_style())
        })
    }
    
    fn ensure_visible_data(&mut self) -> Result<()> {
        debug_log("Checking visible data availability");
        let visible = self.visible_bytes();
        let current_size = self.buffr_collection.current().data.len();
        
        debug_log(&format!("Need bytes up to: {}, have: {}", visible.end, current_size));
        
        if visible.end > current_size {
            debug_log("Need to load more data");
            let current_buffer = self.buffr_collection.current_mut();
            match current_buffer.load_next_chunk(64) {
                Ok(true) => {
                    debug_log(&format!("Loaded chunk. New size: {}", current_buffer.data.len()));
                    Ok(())
                },
                Ok(false) => {
                    debug_log("No more data available");
                    Ok(())
                },
                Err(e) => {
                    debug_log(&format!("Error loading data: {}", e));
                    Err(e)
                }
            }
        } else {
            Ok(())
        }
    }
    

    fn draw_rows(&mut self, stdout: &mut impl Write, invalidated_rows: &BTreeSet<u16>) -> Result<()> {
        
        // Try to load more data if needed
        self.ensure_visible_data()?;
        
        let visible_bytes = self.visible_bytes();
        let start_index = visible_bytes.start;
        let end_index = visible_bytes.end;

        let visible_bytes_cow = self
            .buffr_collection
            .current()
            .data
            .slice_to_cow(start_index..end_index);

        let max_bytes = visible_bytes_cow.len();
        let mark_commands = self.mark_commands(visible_bytes.clone());

        let current_bytes = self
            .buffr_collection
            .current()
            .selection
            .regions_in_range(visible_bytes.start, visible_bytes.end)
            .iter()
            .find(|region| region.is_main())
            .map(|v| {
                let start = v.caret - start_index;
                let end = if start + 4 > visible_bytes_cow.len() {
                    visible_bytes_cow.len()
                } else {
                    start + 4
                };
                &visible_bytes_cow[start..end]
            })
            .unwrap_or_else(|| &[]);

        let mut byte_properties = BytePropertiesFormatter::new(current_bytes);

        for i in visible_bytes.step_by(self.bytes_per_line) {
            if !invalidated_rows.contains(&self.offset_to_row(i).unwrap()) {
                continue;
            }

            let normalized_i = i - start_index;
            let normalized_end = std::cmp::min(max_bytes, normalized_i + self.bytes_per_line);
            self.draw_row(
                stdout,
                &visible_bytes_cow[normalized_i..normalized_end],
                i,
                &mark_commands[normalized_i..normalized_end],
                if i + self.bytes_per_line > self.buffr_collection.current().data.len() {
                    self.overflow_cursor_style()
                } else {
                    None
                },
                &mut byte_properties,
            )?;
        }

        let a = end_index / self.bytes_per_line;
        let mut offset = (if end_index % self.bytes_per_line == 0 {
            a
        } else {
            a + 1
        }) * self.bytes_per_line;
        while !byte_properties.are_all_printed() {
            self.draw_row(stdout, &[], offset, &[], None, &mut byte_properties)?;
            offset += self.bytes_per_line;
        }

        Ok(())
    }

    fn draw(&mut self, stdout: &mut impl Write) -> Result<time::Duration> {
        let begin = time::Instant::now();

        // Try to load more data if needed
        self.ensure_visible_data()?;
        
        queue!(
            stdout,
            cursor::MoveTo(0, 0),
            terminal::Clear(terminal::ClearType::All)
        )?;

        let visible_bytes = self.visible_bytes();
        let start_index = visible_bytes.start;
        let end_index = visible_bytes.end;
        let visible_bytes_cow = self
            .buffr_collection
            .current()
            .data
            .slice_to_cow(start_index..end_index);

        let max_bytes = visible_bytes_cow.len();
        let mark_commands = self.mark_commands(visible_bytes.clone());

        let current_bytes = self
            .buffr_collection
            .current()
            .selection
            .regions_in_range(visible_bytes.start, visible_bytes.end)
            .iter()
            .find(|region| region.is_main())
            .map(|v| {
                let start = v.caret - start_index;
                let end = if start + 4 > visible_bytes_cow.len() {
                    visible_bytes_cow.len()
                } else {
                    start + 4
                };
                &visible_bytes_cow[start..end]
            })
            .unwrap_or_else(|| &[]);

        let mut byte_properties = BytePropertiesFormatter::new(current_bytes);

        for i in visible_bytes.step_by(self.bytes_per_line) {
            let normalized_i = i - start_index;
            let normalized_end = std::cmp::min(max_bytes, normalized_i + self.bytes_per_line);
            self.draw_row(
                stdout,
                &visible_bytes_cow[normalized_i..normalized_end],
                i,
                &mark_commands[normalized_i..normalized_end],
                if i + self.bytes_per_line > self.buffr_collection.current().data.len() {
                    self.overflow_cursor_style()
                } else {
                    None
                },
                &mut byte_properties,
            )?;
        }

        let a = end_index / self.bytes_per_line;
        let mut offset = (if end_index % self.bytes_per_line == 0 {
            a
        } else {
            a + 1
        }) * self.bytes_per_line;
        while !byte_properties.are_all_printed() {
            self.draw_row(stdout, &[], offset, &[], None, &mut byte_properties)?;
            offset += self.bytes_per_line;
        }

        let new_full_rows =
            (end_index - start_index + self.bytes_per_line - 1) / self.bytes_per_line;
        if new_full_rows != self.last_visible_rows.get() {
            self.last_visible_rows.set(new_full_rows);
        }

        self.draw_statusline(stdout)?;

        Ok(begin.elapsed())
    }

    fn handle_event_default(&mut self, stdout: &mut impl Write, event: Event) -> Result<()> {
        match event {
            Event::Resize(x, y) => {
                self.size = (x, y);
                self.draw(stdout)?;
                Ok(())
            }
            Event::Key(KeyEvent { code, modifiers }) => match (code, modifiers) {
                (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                    
                    // Configurable chunk size (e.g., 368 bytes)
                    // default 23 rows x 16 bytes is 368)
                    let (_, height) = terminal::size().unwrap_or((80, 23));
                    let chunk_size = (height as usize - 1) * 16;  // Subtract status line
                    let _ = self.buffr_collection.current_mut().load_next_chunk(chunk_size)?;
                             
                    // In view or wherever scrolling occurs
                    // let _ = self.buffr_collection.current_mut().load_next_chunk(chunk_size)?;
                    let current_buffer = self.buffr_collection.current_mut();
                    let max_bytes = current_buffer.data.len();
                    let bytes_per_line = self.bytes_per_line;

                    current_buffer.map_selections(|region| {
                        vec![region.simple_move(Direction::Down, bytes_per_line, max_bytes, 1)]
                    });

                    self.scroll_down(stdout, 1)?;
                    self.draw(stdout)?;
                    Ok(())
                }
                (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
                    // let _ = self.buffr_collection.current_mut().load_next_chunk(chunk_size)?;
                    let current_buffer = self.buffr_collection.current_mut();
                    let max_bytes = current_buffer.data.len();
                    let bytes_per_line = self.bytes_per_line;

                    current_buffer.map_selections(|region| {
                        vec![region.simple_move(Direction::Up, bytes_per_line, max_bytes, 1)]
                    });

                    self.scroll_up(stdout, 1)?;
                    self.draw(stdout)?;
                    Ok(())
                }
                _ => Ok(()),
            },
            _ => Ok(()),
        }
    }
    

    fn scroll_down(&mut self, stdout: &mut impl Write, line_count: usize) -> Result<()> {
        // Configurable chunk size (e.g., 368 bytes)
        // default 23 rows x 16 bytes is 368)
        let (_, height) = terminal::size().unwrap_or((80, 23));
        let chunk_size = (height as usize - 1) * 16;  // Subtract status line
            
        debug_log("\n=== Scroll Down Event ===");
        debug_log(&format!("scroll_down -> chunk_size -> {}", chunk_size));
        debug_log(&format!("Line count: {}", line_count));
        debug_log(&format!("Current start_offset: {}", self.start_offset));
        
        // let current_buffer = self.buffr_collection.current();
        // let current_size = current_buffer.data.len();
        // debug_log(&format!("Current buffer size: {}", current_size));
        
        // Get current size before modifications
        let current_size = {
            let current_buffer = self.buffr_collection.current();
            current_buffer.data.len()
        };
        debug_log(&format!("Current buffer size: {}", current_size));
        
        let next_position = self.start_offset + (line_count * 16);
        debug_log(&format!("Next position would be: {}", next_position));
        
        // Calculate how many rows we can display
        let visible_rows = (self.size.1 - 1) as usize;  // -1 for status line
        let needed_bytes = next_position + (visible_rows * 16);
        debug_log(&format!("Need bytes up to: {}", needed_bytes));
        
        // // If need more data for full display
        // if needed_bytes > current_size {
        //     debug_log("Loading more data for display");
        //     let current_buffer = self.buffr_collection.current_mut();
        //     match current_buffer.load_next_chunk(64) {
        //         Ok(true) => {
        //             debug_log(&format!("Loaded chunk. New size: {}", current_buffer.data.len()));
        //         },
        //         Ok(false) => debug_log("No more data available"),
        //         Err(e) => debug_log(&format!("Error loading data: {}", e)),
        //     }
        // }

        // Calculate next visible range
        let next_visible_end = next_position + (self.size.1 as usize * self.bytes_per_line);
        let current_size = self.buffr_collection.current().data.len();
        debug_log(&format!("Need bytes up to: {}, have: {}", next_visible_end, current_size));

        // Try to load more data if needed
        if next_visible_end > current_size {
            let current_buffer = self.buffr_collection.current_mut();
            match current_buffer.load_next_chunk(64) {
                Ok(true) => {
                    debug_log(&format!("Loaded chunk. current_buffer.data.len: {}", current_buffer.data.len()));
                    // debug_log(&format!("Should trim? {}", self.should_trim_buffer()));

                    let new_size = current_buffer.data.len();
                    debug_log(&format!("Loaded chunk. New size: {}", new_size));
                    
                    // Log before trim
                    debug_log(&format!("Before trim - start_offset: {}, buffer_size: {}", 
                        self.start_offset, new_size));     
                    
                    // Trim Buffer
                    // if self.should_trim_buffer() {
                    //     debug_log("Trim le Buffeir");
                    //     let (_, height) = terminal::size().unwrap_or((80, 23));
                    //     let chunk_size = (height as usize - 1) * 16;
                    //     self.trim_buffer_top(chunk_size);
                    // }
                    // Check if we should trim (after releasing the borrow)
                    let should_trim = self.should_trim_buffer();
                    if self.should_trim_buffer() {
                        let (_, height) = terminal::size().unwrap_or((80, 23));
                        let chunk_size = (height as usize - 1) * 16;
                        if chunk_size > 0 {  // Only trim if we have a valid chunk size
                            debug_log(&format!("Trimming with chunk_size: {}", chunk_size));
                            self.trim_buffer_top(chunk_size);
                        } else {
                            debug_log("Window too small for trimming");
                        }
                    }
                    
                    // if should_trim {
                    //     debug_log("Trimming buffer...");
                    //     let old_start = self.start_offset;
                    //     self.trim_buffer_top(chunk_size);
                        
                    //     // Get new size after trim
                    //     let new_size = self.buffr_collection.current().data.len();
                    //     debug_log(&format!("After trim - start_offset: {} -> {}, buffer_size: {}", 
                    //         old_start, self.start_offset, new_size));
                    // }

                },
                Ok(false) => {
                    debug_log("No more data available");
                    if next_position >= current_size {
                        return Ok(());
                    }
                },
                Err(e) => {
                    debug_log(&format!("Error loading data: {}", e));
                    return Err(e);
                }
            }
        }
            
        // // Check if we can scroll to the next position
        // if next_position >= self.buffr_collection.current().data.len() {
        //     debug_log("Cannot scroll further - at end of file");
        //     return Ok(());
        // }

        // Check buffer size again after potential modifications
        let final_size = self.buffr_collection.current().data.len();
        debug_log(&format!("Final buffer check - size: {}, next_position: {}", 
            final_size, next_position));
                
        if next_position >= final_size {
            debug_log("Cannot scroll further - at end of file");
            return Ok(());
        }
        
        // // If we get here, we can safely scroll
        // self.start_offset = next_position;
        // debug_log(&format!("Scrolled to new start_offset: {}", self.start_offset));

        // Update start_offset
        let old_start = self.start_offset;
        self.start_offset = next_position;
        debug_log(&format!("Updated start_offset: {} -> {}", old_start, self.start_offset));

        if line_count > (self.size.1 - 1) as usize {
            self.draw(stdout)?;
            Ok(())
        } else {
            queue!(
                stdout,
                terminal::ScrollUp(line_count as u16),
                cursor::MoveTo(0, self.size.1 - 2),
                terminal::Clear(terminal::ClearType::CurrentLine),
            )?;

            let mut invalidated_rows: BTreeSet<u16> =
                (self.size.1 - 1 - line_count as u16..=self.size.1 - 2).collect();
            invalidated_rows.extend(0..BytePropertiesFormatter::height() as u16);
            let _ = self.draw_rows(stdout, &invalidated_rows)?;
            Ok(())
        }
    }

    fn scroll_up(&mut self, stdout: &mut impl Write, line_count: usize) -> Result<()> {
        if self.start_offset < 0x10 * line_count {
            // we already at the top the file
            return Ok(());
        }

        self.start_offset -= 0x10 * line_count;

        if line_count > (self.size.1 - 1) as usize {
            self.draw(stdout)?;
            Ok(())
        } else {
            queue!(
                stdout,
                terminal::ScrollDown(line_count as u16),
                cursor::MoveTo(0, self.size.1 - 1),
                terminal::Clear(terminal::ClearType::CurrentLine),
            )?;

            let invalidated_rows: BTreeSet<u16> =
                (0..(line_count + BytePropertiesFormatter::height()) as u16).collect();
            self.draw_rows(stdout, &invalidated_rows) // -1 is statusline
        }
    }

    fn maybe_update_offset(&mut self, stdout: &mut impl Write) -> Result<()> {
        if self.buffr_collection.current().data.is_empty() {
            self.start_offset = 0;
            return Ok(());
        }

        let main_cursor_offset = self.buffr_collection.current().selection.main_cursor_offset();
        let visible_bytes = self.visible_bytes();
        let delta = if main_cursor_offset < visible_bytes.start {
            main_cursor_offset as isize - visible_bytes.start as isize
        } else if main_cursor_offset >= visible_bytes.end {
            main_cursor_offset as isize - (visible_bytes.end as isize - 1)
        } else {
            return Ok(());
        };
        if delta < 0 {
            let line_delta =
                (delta - self.bytes_per_line as isize + 1) / self.bytes_per_line as isize;
            self.scroll_up(stdout, line_delta.abs() as usize)
        } else {
            let line_delta =
                (delta + self.bytes_per_line as isize - 1) / self.bytes_per_line as isize;
            self.scroll_down(stdout, line_delta as usize)
        }
    }

    fn maybe_update_offset_and_draw(&mut self, stdout: &mut impl Write) -> Result<()> {
        let main_cursor_offset = self.buffr_collection.current().selection.main_cursor_offset();
        let visible_bytes = self.visible_bytes();
        if main_cursor_offset < visible_bytes.start {
            self.start_offset = main_cursor_offset - main_cursor_offset % self.bytes_per_line;
        } else if main_cursor_offset >= visible_bytes.end {
            let bytes_per_screen = (self.size.1 as usize - 1) * self.bytes_per_line; // -1 for statusline
            self.start_offset = (main_cursor_offset - main_cursor_offset % self.bytes_per_line
                + self.bytes_per_line)
                .saturating_sub(bytes_per_screen);
        }

        self.draw(stdout)?;
        Ok(())
    }

    fn transition_dirty_bytes(
        &mut self,
        stdout: &mut impl Write,
        dirty_bytes: DirtyBytes,
    ) -> Result<()> {
        match dirty_bytes {
            DirtyBytes::ChangeInPlace(intervals) => {
                self.maybe_update_offset(stdout)?;

                let visible: Interval = self.visible_bytes().into();
                let mut invalidated_rows: BTreeSet<u16> = intervals
                    .into_iter()
                    .flat_map(|x| {
                        let intersection = visible.intersect(x);
                        if intersection.is_empty() {
                            0..0
                        } else {
                            intersection.start..intersection.end
                        }
                    })
                    .map(|byte| ((byte - self.start_offset) / self.bytes_per_line) as u16)
                    .collect();

                invalidated_rows.extend(0..BytePropertiesFormatter::height() as u16);
                self.draw_rows(stdout, &invalidated_rows)
            }
            DirtyBytes::ChangeLength => self.maybe_update_offset_and_draw(stdout),
        }
    }

    fn transition(&mut self, stdout: &mut impl Write, transition: ModeTransition) -> Result<()> {
        self.info = None;
        match transition {
            ModeTransition::None => Ok(()),
            ModeTransition::DirtyBytes(dirty_bytes) => {
                self.transition_dirty_bytes(stdout, dirty_bytes)
            }
            ModeTransition::NewMode(mode) => {
                self.mode = mode;
                Ok(())
            }
            ModeTransition::ModeAndDirtyBytes(mode, dirty_bytes) => {
                self.mode = mode;
                self.transition_dirty_bytes(stdout, dirty_bytes)
            }
            ModeTransition::ModeAndInfo(mode, info) => {
                self.mode = mode;
                self.info = Some(info);
                Ok(())
            }
        }
    }

    pub fn run_event_loop(mut self, stdout: &mut impl Write) -> Result<()> {
        execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;

        self.last_draw_time = self.draw(stdout)?;
        terminal::enable_raw_mode()?;
        stdout.flush()?;

        loop {
            if !self.mode.takes_input() {
                break;
            }
            let evt = event::read()?;
            let transition = self
                .mode
                .transition(&evt, &mut self.buffr_collection, self.bytes_per_line);
            if let Some(transition) = transition {
                self.transition(stdout, transition)?;
            } else {
                self.handle_event_default(stdout, evt)?;
            }

            self.draw_statusline(stdout)?;
            stdout.flush()?;
        }
        execute!(stdout, cursor::Show, terminal::LeaveAlternateScreen)?;
        terminal::disable_raw_mode()?;
        Ok(())
    }
}
