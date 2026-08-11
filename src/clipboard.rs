use std::io::{self, Write};

use base64::{Engine as _, engine::general_purpose::STANDARD};

/// Copies text through the terminal using OSC 52.
///
/// This keeps clipboard support portable and allows compatible terminals to
/// copy from remote sessions without requiring platform-specific utilities.
pub fn copy(text: &str) -> io::Result<()> {
    let sequence = osc52_sequence(text);
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    stdout.write_all(sequence.as_bytes())?;
    stdout.flush()
}

fn osc52_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_clipboard_text_as_osc52() {
        assert_eq!(osc52_sequence("abc123"), "\x1b]52;c;YWJjMTIz\x07");
    }
}
