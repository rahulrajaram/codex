# Codex Voice Integration Setup

This directory contains the voice integration scripts for Codex TUI, allowing you to record speech and have it transcribed directly into the chat composer.

## Quick Start

1. **Install Python dependencies:**
   ```bash
   cd scripts/voice
   pip install -r requirements.txt
   ```

2. **Test voice integration:**
   ```bash
   # From the codex root directory
   ./target/debug/codex
   # Press F9 to start/stop recording
   ```

## Files

- `whisper` - Main wrapper script used by Codex TUI
- `whisper_cli.py` - Python implementation with FIFO support
- `requirements.txt` - Python dependencies
- `README.md` - Original whisper documentation
- `SETUP.md` - This setup guide

## How It Works

1. **F9 Key** - Toggle voice recording in Codex TUI
2. **FIFO Communication** - Clean transcript delivery via named pipes
3. **Signal Handling** - Proper process management with SIGINT/SIGTERM
4. **Auto-Detection** - TUI automatically finds bundled scripts

## Configuration

The TUI will automatically use bundled scripts if available. You can override with environment variables:

- `CODEX_VOICE_WHISPER_EXE` - Full path to whisper executable
- `CODEX_VOICE_WHISPER_DIR` - Directory containing whisper scripts
- `CODEX_VOICE_DEBUG` - Enable debug logging (set to `1`)
- `CODEX_VOICE_IGNORE_STDERR` - Ignore stderr output (set to `1`)

## Troubleshooting

1. **No audio device:**
   ```bash
   python3 whisper_cli.py --configure
   ```

2. **Permission errors:**
   ```bash
   chmod +x scripts/voice/whisper
   ```

3. **Dependencies missing:**
   ```bash
   pip install pyaudio whisper torch
   ```

4. **Debug mode:**
   ```bash
   CODEX_VOICE_DEBUG=1 ./target/debug/codex
   ```

## Requirements

- Python 3.8+
- PyAudio (for microphone access)
- OpenAI Whisper (for speech recognition)
- PyTorch (Whisper dependency)

For detailed setup instructions, see `README.md` in this directory.