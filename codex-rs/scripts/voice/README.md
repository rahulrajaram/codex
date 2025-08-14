# Whisper CLI - Real-time Speech-to-Text

A real-time speech-to-text CLI tool using OpenAI's Whisper, with support for REPL integration.

## Quick Start

```bash
# Install dependencies
./setup.sh

# Run standalone CLI
./whisper

# Or run without ALSA warnings
python whisper_cli.py 2>/dev/null
```

## Features

- ✅ Real-time audio recording from microphone
- ✅ OpenAI Whisper integration for speech-to-text
- ✅ Interactive CLI with ENTER to start/stop recording
- ✅ Microphone selection from available input devices
- ✅ Clean output without ALSA/JACK warnings
- ✅ **REPL integration for Claude CLI, Codex, etc.**
- ✅ Persistent microphone selection

## REPL Integration

### Option 1: Simple Function Integration

Add to your existing CLI:

```python
# Add to your CLI imports
from whisper_recorder import handle_voice_command

# In your command handler
if user_input == '/voice':
    voice_text = handle_voice_command()
    if voice_text:
        # Process voice_text as if user typed it
        self.process_input(voice_text)
```

### Option 2: Drop-in Claude CLI Integration

```python
# In your Claude CLI main loop
if user_input == '/voice':
    from claude_integration import handle_voice_command
    voice_text = handle_voice_command()
    if voice_text:
        user_input = voice_text
        # Continue processing as normal
```

### Option 3: Proxy Mode

Run as a proxy that adds voice to any CLI:

```bash
# For Claude CLI
python claude_integration.py

# For general example
python integration_example.py
```

## Voice Commands in REPLs

Once integrated, you can use these shortcuts:

- `/voice` - Toggle recording (start/stop)
- `/mic` - Select microphone
- `/help` - Show help

## Workflow

1. **First time**: Select your microphone (saved to `~/.whisper_config.json`)
2. **In REPL**: Type `/voice` to start recording
3. **Speak**: Say what you want to transcribe
4. **Stop**: Type `/voice` again to stop and transcribe
5. **Result**: Transcribed text is sent to your CLI as if you typed it

## Configuration

- Microphone selection is persistent (saved to `~/.whisper_config.json`)
- Model size can be configured (`tiny`, `base`, `small`, `medium`, `large`)
- Audio settings are optimized for real-time use

## Files

- `whisper_cli.py` - Standalone CLI application
- `whisper` - Clean wrapper script (suppresses ALSA warnings)
- `whisper_recorder.py` - Reusable module for integration
- `claude_integration.py` - Drop-in Claude CLI integration
- `integration_example.py` - Example REPL with voice support

## Requirements

- Python 3.7+
- PyAudio (for microphone access)
- OpenAI Whisper
- NumPy

## Installation

```bash
# Clone or download this directory
cd whisper

# Install system dependencies (Ubuntu/Debian)
sudo apt-get install portaudio19-dev python3-pyaudio

# Install Python dependencies
pip install -r requirements.txt

# Test the installation
./whisper
```

## Troubleshooting

### ALSA Warnings
Use the wrapper script: `./whisper` or redirect stderr: `python whisper_cli.py 2>/dev/null`

### Microphone Issues
1. Run `./whisper` and select your microphone
2. Check `python -c "import pyaudio; pa = pyaudio.PyAudio(); print(pa.get_device_count())"`
3. Ensure microphone permissions are granted

### Integration Issues
1. Make sure `whisper_recorder.py` is in your Python path
2. Test with `python integration_example.py` first
3. Check that your CLI can import the integration modules

## Examples

### Standalone Use
```bash
$ ./whisper
🤖 Loading Whisper model...

🎤 Available input devices:
  0: Built-in Microphone (DEFAULT)
  1: USB Headset

Select microphone (0-1, or press ENTER for default): 1
🎯 Selected: USB Headset

🎙️  Whisper Real-time CLI
==============================

Press ENTER to start recording (or 'quit' to exit): 
🎤 Recording started... Press ENTER to stop.

⏹️  Recording stopped. Processing...
🤖 Transcribing...

📝 Transcription:
   Hello, this is a test of the whisper speech to text system.
```

### In Claude CLI
```bash
claude> /voice
🎤 Starting recording... (type /voice again to stop)

claude> /voice
⏹️  Stopping recording...
📝 Transcribed: Write a Python function to calculate fibonacci numbers
➤ Sending to Claude: Write a Python function to calculate fibonacci numbers

[Claude processes the request as if you typed it]
```