#![deny(clippy::all)]
use std::fs::File;
use std::io::Read;
use std::io::{stdout, BufWriter};
use teehee::hex_view::view::HexView;
use teehee::{CurrentBuffer, BuffrCollection};
use std::fs::OpenOptions;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use std::io::Write;
use crossterm::terminal;
use std::env;

const STDOUT_BUF: usize = 8192;

fn debug_log(message: &str) {
    // Get the current working directory
    let cwd = env::current_dir().expect("Failed to get current directory");
    let log_path = cwd.join("teehee_debug.log");

    // Testprint
    // println!("Attempting to log to: {}", log_path.display());

    // Check if path exists and is writable
    // println!("Path exists: {}", log_path.exists());

    match OpenOptions::new()
        .create(true)
        .write(true)
        .append(true)
        .open(&log_path)
    {
        Ok(mut file) => {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_secs();

            match writeln!(file, "[{}] {}", timestamp, message) {
                Ok(_) => println!("Log written successfully to {}", log_path.display()),
                Err(e) => println!("Failed to write to log: {}", e),
            }
        }
        Err(e) => println!("Failed to open log file: {}", e),
    }
}

fn main() {
    debug_log("Starting teehee");


    let stdout = stdout();
    let mut stdout = BufWriter::with_capacity(STDOUT_BUF, stdout.lock());
    let filename = std::env::args().nth(1);
    
    // Load only a window_chunk
    let buffr_collection = filename
        .as_ref()
        .map(|filename| {
            debug_log(&format!("Attempting to load file: {:?}", filename));
            // Open file and read only first chunk
            let mut file = File::open(filename).expect("Couldn't open file");
            let mut current_buffer = Vec::new();
            
            // Configurable chunk size (e.g., 368 bytes)
            // default 23 rows x 16 bytes is 368)
            let (_, height) = terminal::size().unwrap_or((80, 23));
            let chunk_size = (height as usize - 1) * 16;  // Subtract status line
            

            debug_log(&format!("Loading file with chunk size: {}", chunk_size));
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
