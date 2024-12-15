#![deny(clippy::all)]
use std::fs::File;
use std::io::Read;
use std::io::{stdout, BufWriter};
use teehee::hex_view::view::HexView;
use teehee::{CurrentBuffer, BuffrCollection};

use crossterm::terminal;



const STDOUT_BUF: usize = 8192;

fn main() {
    let stdout = stdout();
    let mut stdout = BufWriter::with_capacity(STDOUT_BUF, stdout.lock());
    let filename = std::env::args().nth(1);
    
    // Load only a window_chunk
    let buffr_collection = filename
        .as_ref()
        .map(|filename| {
            // Open file and read only first chunk
            let mut file = File::open(filename).expect("Couldn't open file");
            let mut current_buffer = Vec::new();
            
            // Configurable chunk size (e.g., 368 bytes)
            // default 23 rows x 16 bytes is 368)
            let (_, height) = terminal::size().unwrap_or((80, 23));
            let chunk_size = (height as usize - 1) * 16;  // Subtract status line
            // let chunk_size = 368;
            current_buffer.resize(chunk_size, 0);
            
            let bytes_read = file.read(&mut current_buffer).expect("Couldn't read file");
            current_buffer.truncate(bytes_read);
    
            BuffrCollection::with_current_buffer(CurrentBuffer::from_data_and_path(
                current_buffer,
                Some(filename),
            ))
        })
        .unwrap_or_else(BuffrCollection::new);

    /*
    Original, loads whole file
    */
    // let buffr_collection = filename
    //     .as_ref()
    //     .map(|filename| {
    //         BuffrCollection::with_current_buffer(CurrentBuffer::from_data_and_path(
    //             std::fs::read(&filename).expect("Couldn't read file"),
    //             Some(filename),
    //         ))
    //     })
    //     .unwrap_or_else(BuffrCollection::new);
        
        
    let view = HexView::with_buffr_collection(buffr_collection);

    view.run_event_loop(&mut stdout).unwrap();
}
