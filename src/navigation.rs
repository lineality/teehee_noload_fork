/// # Navigation System for Binary File Viewing
/// 
/// Provides percentage-based navigation through large binary files.
/// Supports both absolute and relative positioning with bounds checking
/// and proper buffer management.
/// 
/// ## Features
/// - Absolute percentage jumps (e.g., 50%)
/// - Relative moves (e.g., +10%, -5%)
/// - Quick markers (START, QUARTER, MIDDLE, THREE_QUARTERS, END)
/// 
/// ## Usage Examples
/// ```text
/// :50%     - Jump to middle
/// :+10%    - Move forward 10%
/// :-5%     - Move back 5%
/// :start   - Jump to beginning
/// :end     - Jump to end
/// ```
pub struct NavigationSystem {
    file_size: u64,
    current_percentage: f64,
    chunk_size: usize,
}

/// Standard position markers for quick navigation
pub enum FilePosition {
    START,           // 0%
    QUARTER,         // 25%
    MIDDLE,         // 50%
    THREE_QUARTERS, // 75%
    END,            // 100%
}

impl NavigationSystem {
    /// Creates a new navigation system for the given file
    pub fn new(file_path: &Path) -> Result<Self, std::io::Error> {
        let file_size = std::fs::metadata(file_path)?.len();
        Ok(Self {
            file_size,
            current_percentage: 0.0,
            chunk_size: 384, // Default view size
        })
    }

    /// Jump to an absolute percentage position in the file
    /// 
    /// # Arguments
    /// * `percentage` - Target position (0.0 to 100.0)
    /// 
    /// # Returns
    /// * `Ok(u64)` - The calculated offset
    /// * `Err` - If percentage is invalid
    pub fn jump_to_percentage(&mut self, percentage: f64) -> Result<u64, std::io::Error> {
        debug_log(&format!("Attempting jump to {}%", percentage));

        if !(0.0..=100.0).contains(&percentage) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Percentage must be between 0 and 100"
            ));
        }

        let target_offset = self.percentage_to_offset(percentage);
        self.current_percentage = percentage;

        debug_log(&format!(
            "Jump calculated - File size: {}, Target offset: {}", 
            self.file_size, target_offset
        ));

        Ok(target_offset)
    }

    /// Move relative to current position by percentage
    /// 
    /// # Arguments
    /// * `delta_percentage` - Amount to move (+/-), e.g., +10.0 or -5.0
    pub fn move_relative(&mut self, delta_percentage: f64) -> Result<u64, std::io::Error> {
        let new_percentage = (self.current_percentage + delta_percentage)
            .max(0.0)
            .min(100.0);
        
        self.jump_to_percentage(new_percentage)
    }

    /// Jump to a predefined position in the file
    pub fn jump_to_position(&mut self, position: FilePosition) -> Result<u64, std::io::Error> {
        let percentage = match position {
            FilePosition::START => 0.0,
            FilePosition::QUARTER => 25.0,
            FilePosition::MIDDLE => 50.0,
            FilePosition::THREE_QUARTERS => 75.0,
            FilePosition::END => 100.0,
        };
        
        self.jump_to_percentage(percentage)
    }

    /// Calculate current viewing window
    pub fn get_current_window(&self) -> (u64, u64) {
        let current_offset = self.percentage_to_offset(self.current_percentage);
        let window_end = (current_offset + self.chunk_size as u64)
            .min(self.file_size);
        
        (current_offset, window_end)
    }

    /// Get human-readable position information
    pub fn get_position_info(&self) -> String {
        let (start, end) = self.get_current_window();
        format!(
            "Position: {:.1}% (Offset: {}/{} bytes)",
            self.current_percentage,
            start,
            self.file_size
        )
    }

    // Helper methods
    fn percentage_to_offset(&self, percentage: f64) -> u64 {
        ((percentage / 100.0) * self.file_size as f64) as u64
    }
}

/// # Command Parser for Navigation
/// 
/// Handles user input for navigation commands
pub struct NavigationCommand {
    pub command_type: NavCommandType,
    pub value: f64,
}

pub enum NavCommandType {
    AbsoluteJump,
    RelativeMove,
    QuickPosition(FilePosition),
}

impl NavigationCommand {
    /// Parse user input into navigation command
    /// 
    /// # Examples
    /// ```text
    /// "50%"    -> AbsoluteJump(50.0)
    /// "+10%"   -> RelativeMove(10.0)
    /// "-5%"    -> RelativeMove(-5.0)
    /// "start"  -> QuickPosition(START)
    /// "end"    -> QuickPosition(END)
    /// ```
    pub fn parse(input: &str) -> Result<Self, &'static str> {
        let input = input.trim().to_lowercase();
        
        // Handle quick positions
        match input.as_str() {
            "start" => return Ok(Self {
                command_type: NavCommandType::QuickPosition(FilePosition::START),
                value: 0.0,
            }),
            "end" => return Ok(Self {
                command_type: NavCommandType::QuickPosition(FilePosition::END),
                value: 0.0,
            }),
            _ => {}
        }

        // Handle percentage moves
        if let Some(percentage) = input.strip_suffix('%') {
            if let Some(value) = percentage.strip_prefix('+') {
                // Relative positive move
                if let Ok(num) = value.parse::<f64>() {
                    return Ok(Self {
                        command_type: NavCommandType::RelativeMove,
                        value: num,
                    });
                }
            } else if let Some(value) = percentage.strip_prefix('-') {
                // Relative negative move
                if let Ok(num) = value.parse::<f64>() {
                    return Ok(Self {
                        command_type: NavCommandType::RelativeMove,
                        value: -num,
                    });
                }
            } else {
                // Absolute jump
                if let Ok(num) = percentage.parse::<f64>() {
                    return Ok(Self {
                        command_type: NavCommandType::AbsoluteJump,
                        value: num,
                    });
                }
            }
        }

        Err("Invalid navigation command")
    }
}