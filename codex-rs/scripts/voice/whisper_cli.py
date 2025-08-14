#!/usr/bin/env python3
import pyaudio
import wave
import whisper
import threading
import tempfile
import os
import signal
import sys
import time
import argparse
import json
import itertools
from datetime import datetime
from contextlib import redirect_stderr
import io

class WhisperCLI:
    def __init__(self, headless=False, force_configure=False):
        self.headless = headless
        
        # Suppress all ALSA/JACK warnings globally
        self._suppress_audio_warnings()
        self.config_file = os.path.expanduser("~/.whisper/config")
        self.spinner_running = False
        self.spinner_thread = None
        
        if not headless:
            print("🤖 Loading Whisper model...")
        self.model = whisper.load_model("base")
        self.recording = False
        self.audio_data = []
        self.stream = None
        self.audio = None
        
        # Audio settings
        self.chunk = 4096  # Larger chunk size
        self.format = pyaudio.paInt16
        self.channels = 1
        self.rate = 16000
        self.input_device_index = None
        
        # Initialize PyAudio with error handling
        self._init_audio()
        
        # Load or configure microphone
        if force_configure or not self._load_config():
            self._select_microphone()
            self._save_config()
        
        # Set up signal handlers for clean exit
        signal.signal(signal.SIGINT, self.signal_handler)
        signal.signal(signal.SIGTERM, self.signal_handler)
    
    def _suppress_audio_warnings(self):
        """Set environment variables to reduce ALSA warnings"""
        os.environ['ALSA_PCM_CARD'] = 'default'
        os.environ['ALSA_PCM_DEVICE'] = '0'
        # Additional suppression for headless mode
        if self.headless:
            os.environ['JACK_NO_AUDIO_RESERVATION'] = '1'
            os.environ['PULSE_LATENCY_MSEC'] = '30'
    
    def _init_audio(self):
        """Initialize PyAudio with proper error handling"""
        try:
            # Suppress ALSA warnings during PyAudio initialization
            if self.headless:
                # In headless mode, suppress all stderr output during init
                with open(os.devnull, 'w') as devnull:
                    with redirect_stderr(devnull):
                        self.audio = pyaudio.PyAudio()
            else:
                with redirect_stderr(io.StringIO()):
                    self.audio = pyaudio.PyAudio()
            
        except Exception as e:
            if not self.headless:
                print(f"Error initializing audio: {e}")
                print("Please check your microphone connection and permissions.")
            sys.exit(1)
    
    def _load_config(self):
        """Load configuration from file"""
        try:
            if os.path.exists(self.config_file):
                with open(self.config_file, 'r') as f:
                    config = json.load(f)
                    self.input_device_index = config.get('input_device_index')
                    return True
        except Exception as e:
            if not self.headless:
                print(f"Warning: Could not load config: {e}")
        return False
    
    def _save_config(self):
        """Save configuration to file"""
        try:
            os.makedirs(os.path.dirname(self.config_file), exist_ok=True)
            config = {
                'input_device_index': self.input_device_index
            }
            with open(self.config_file, 'w') as f:
                json.dump(config, f, indent=2)
        except Exception as e:
            if not self.headless:
                print(f"Warning: Could not save config: {e}")
    
    def _select_microphone(self):
        """Let user select which microphone to use"""
        if self.headless:
            # In headless mode, automatically use the default input device
            try:
                self.input_device_index = None
                return
            except Exception:
                # If no default device, exit silently in headless mode
                sys.exit(1)
        
        input_devices = []
        
        print("\n🎤 Available input devices:")
        
        # Get all input devices
        for i in range(self.audio.get_device_count()):
            try:
                device_info = self.audio.get_device_info_by_index(i)
                if device_info['maxInputChannels'] > 0:
                    input_devices.append((i, device_info))
                    default_marker = " (DEFAULT)" if i == self.audio.get_default_input_device_info()['index'] else ""
                    print(f"  {len(input_devices)-1}: {device_info['name']}{default_marker}")
            except:
                continue
        
        if not input_devices:
            print("No input devices found!")
            sys.exit(1)
        
        # Let user choose
        while True:
            try:
                choice = input(f"\nSelect microphone (0-{len(input_devices)-1}, or press ENTER for default): ").strip()
                
                if choice == "":
                    # Use default device
                    self.input_device_index = None
                    default_device = self.audio.get_default_input_device_info()
                    print(f"🎯 Using default input device: {default_device['name']}")
                    break
                
                choice_idx = int(choice)
                if 0 <= choice_idx < len(input_devices):
                    device_idx, device_info = input_devices[choice_idx]
                    self.input_device_index = device_idx
                    print(f"🎯 Selected: {device_info['name']}")
                    break
                else:
                    print(f"Please enter a number between 0 and {len(input_devices)-1}")
                    
            except ValueError:
                print("Please enter a valid number")
            except KeyboardInterrupt:
                print("\nExiting...")
                sys.exit(0)
    
    def signal_handler(self, sig, frame):
        print(f"DEBUG: Signal {sig} received", file=sys.stderr)
        if not self.headless:
            print("\nExiting...")
        
        # If we're currently recording, stop and process the audio
        if self.recording:
            print("DEBUG: Was recording, stopping and processing audio", file=sys.stderr)
            self._stop_spinner()
            transcript = self.stop_recording()
            print(f"DEBUG: Got transcript from stop_recording: '{transcript}' (length: {len(transcript) if transcript else 0})", file=sys.stderr)
            # Write to FIFO even if interrupted
            self._write_to_fifo(transcript)
        else:
            print("DEBUG: Was not recording", file=sys.stderr)
        
        self.cleanup()
        sys.exit(0)
    
    def start_recording(self):
        if self.recording:
            return
        
        try:
            self.recording = True
            self.audio_data = []
            
            if not self.headless:
                print("🎤 Recording started... Press ENTER to stop.")
            
            # Open audio stream with error handling (suppress ALSA warnings)
            if self.headless:
                # In headless mode, suppress all stderr output during stream creation
                with open(os.devnull, 'w') as devnull:
                    with redirect_stderr(devnull):
                        self.stream = self.audio.open(
                            format=self.format,
                            channels=self.channels,
                            rate=self.rate,
                            input=True,
                            frames_per_buffer=self.chunk,
                            input_device_index=self.input_device_index,
                            start=False  # Don't start immediately
                        )
                        # Start the stream
                        self.stream.start_stream()
            else:
                with redirect_stderr(io.StringIO()):
                    self.stream = self.audio.open(
                        format=self.format,
                        channels=self.channels,
                        rate=self.rate,
                        input=True,
                        frames_per_buffer=self.chunk,
                        input_device_index=self.input_device_index,
                        start=False  # Don't start immediately
                    )
                    # Start the stream
                    self.stream.start_stream()
            
            # Start recording in a separate thread
            self.recording_thread = threading.Thread(target=self._record_audio, daemon=True)
            self.recording_thread.start()
            
        except Exception as e:
            if not self.headless:
                print(f"Error starting recording: {e}")
            self.recording = False
            if self.stream:
                try:
                    self.stream.close()
                except:
                    pass
                self.stream = None
    
    def _record_audio(self):
        while self.recording and self.stream:
            try:
                if self.stream.is_active():
                    data = self.stream.read(self.chunk, exception_on_overflow=False)
                    self.audio_data.append(data)
                else:
                    time.sleep(0.01)  # Small delay to prevent busy waiting
            except OSError as e:
                print(f"Audio stream error: {e}")
                self.recording = False
                break
            except Exception as e:
                print(f"Unexpected error in recording: {e}")
                self.recording = False
                break
    
    def stop_recording(self):
        print("DEBUG: stop_recording() called", file=sys.stderr)
        if not self.recording:
            print("DEBUG: not recording, returning None", file=sys.stderr)
            return
        
        self.recording = False
        print("DEBUG: set recording=False, processing audio", file=sys.stderr)
        if not self.headless:
            print("⏹️  Recording stopped. Processing...")
        
        # Wait for recording thread to finish
        if hasattr(self, 'recording_thread') and self.recording_thread.is_alive():
            print("DEBUG: waiting for recording thread to finish", file=sys.stderr)
            self.recording_thread.join(timeout=2.0)
            print("DEBUG: recording thread finished", file=sys.stderr)
        
        # Clean up stream
        if self.stream:
            try:
                if self.stream.is_active():
                    print("DEBUG: stopping audio stream", file=sys.stderr)
                    self.stream.stop_stream()
                print("DEBUG: closing audio stream", file=sys.stderr)
                self.stream.close()
            except Exception as e:
                print(f"DEBUG: error closing stream: {e}", file=sys.stderr)
                if not self.headless:
                    print(f"Error closing stream: {e}")
            finally:
                self.stream = None
        
        # Save audio to temporary file and transcribe
        print("DEBUG: calling _transcribe_audio()", file=sys.stderr)
        result = self._transcribe_audio()
        print(f"DEBUG: _transcribe_audio() returned: '{result}' (length: {len(result) if result else 0})", file=sys.stderr)
        return result
    
    def _transcribe_audio(self):
        print(f"DEBUG: _transcribe_audio() called, audio_data length: {len(self.audio_data) if self.audio_data else 0}", file=sys.stderr)
        if not self.audio_data:
            print("DEBUG: no audio data recorded", file=sys.stderr)
            if not self.headless:
                print("No audio data recorded.")
            return None
        
        # Create temporary wav file
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as temp_file:
            temp_filename = temp_file.name
            
            # Write audio data to wav file
            wf = wave.open(temp_filename, 'wb')
            wf.setnchannels(self.channels)
            wf.setsampwidth(self.audio.get_sample_size(self.format))
            wf.setframerate(self.rate)
            wf.writeframes(b''.join(self.audio_data))
            wf.close()
        
        try:
            # Transcribe with Whisper
            if not self.headless:
                print("🤖 Transcribing...")
            
            # Suppress warnings during transcription in headless mode
            if self.headless:
                import warnings
                with warnings.catch_warnings():
                    warnings.simplefilter("ignore")
                    with open(os.devnull, 'w') as devnull:
                        with redirect_stderr(devnull):
                            result = self.model.transcribe(temp_filename)
            else:
                result = self.model.transcribe(temp_filename)
            
            # Display results
            text = result["text"].strip()
            if text:
                if self.headless:
                    print(text)
                else:
                    print(f"\n📝 Transcription:")
                    print(f"   {text}")
                
                # Write to FIFO if environment variable is set
                self._write_to_fifo(text)
                
                return text
            else:
                if not self.headless:
                    print("   (No speech detected)")
                return None
            
        except Exception as e:
            if not self.headless:
                print(f"Error during transcription: {e}")
            return None
        finally:
            # Clean up temporary file
            try:
                os.unlink(temp_filename)
            except:
                pass
    
    def _write_to_fifo(self, text):
        """Write transcript to FIFO if environment variable is set"""
        fifo_path = os.environ.get('WHISPER_TRANSCRIPT_FIFO')
        if fifo_path:
            try:
                # Debug: log FIFO write attempt
                print(f"DEBUG: Attempting to write to FIFO {fifo_path}, text length: {len(text) if text else 0}", file=sys.stderr)
                if text:
                    with open(fifo_path, 'w') as f:
                        f.write(text)
                        f.flush()  # Ensure data is written immediately
                    print(f"DEBUG: Successfully wrote {len(text)} chars to FIFO", file=sys.stderr)
                else:
                    print("DEBUG: No text to write to FIFO", file=sys.stderr)
            except Exception as e:
                print(f"DEBUG: FIFO write error: {e}", file=sys.stderr)
                if not self.headless:
                    print(f"Warning: Could not write to FIFO {fifo_path}: {e}")
        else:
            print("DEBUG: No FIFO path set in environment", file=sys.stderr)

    def cleanup(self):
        if self.recording:
            self.stop_recording()
        
        if self.audio:
            try:
                self.audio.terminate()
            except Exception as e:
                print(f"Error cleaning up audio: {e}")
            finally:
                self.audio = None
    
    def _show_spinner(self):
        """Show a spinner animation while waiting for input"""
        spinner = itertools.cycle(['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧'])
        while self.spinner_running:
            sys.stdout.write(f'\r{next(spinner)} Recording... Press ENTER to stop')
            sys.stdout.flush()
            time.sleep(0.1)
        sys.stdout.write('\r' + ' ' * 40 + '\r')  # Clear the line
        sys.stdout.flush()
    
    def _start_spinner(self):
        """Start the spinner in a separate thread"""
        if self.headless:
            self.spinner_running = True
            self.spinner_thread = threading.Thread(target=self._show_spinner, daemon=True)
            self.spinner_thread.start()
    
    def _stop_spinner(self):
        """Stop the spinner"""
        if self.headless and self.spinner_running:
            self.spinner_running = False
            if self.spinner_thread and self.spinner_thread.is_alive():
                self.spinner_thread.join(timeout=0.5)
    
    def run_headless(self):
        """Run in headless mode - single record/transcribe operation"""
        try:
            self.start_recording()
            self._start_spinner()
            # Wait for ENTER or Ctrl+C
            input()
            self._stop_spinner()
            return self.stop_recording()
        except KeyboardInterrupt:
            self._stop_spinner()
            self.stop_recording()
            return None
        finally:
            self._stop_spinner()
            self.cleanup()
    
    def run(self):
        if self.headless:
            return self.run_headless()
        
        print("🎙️  Whisper Real-time CLI")
        print("=" * 30)
        print("Commands:")
        print("  ENTER - Start/Stop recording")
        print("  'quit' - Exit")
        print("=" * 30)
        
        try:
            while True:
                user_input = input("\nPress ENTER to start recording (or 'quit' to exit): ").strip()
                
                if user_input.lower() in ['quit', 'exit', 'q']:
                    break
                
                if not self.recording:
                    self.start_recording()
                    # Wait for next input to stop recording
                    input()  # This will block until ENTER is pressed
                    self.stop_recording()
                
        except KeyboardInterrupt:
            pass
        finally:
            self.cleanup()
            if not self.headless:
                print("\nGoodbye!")

def main():
    parser = argparse.ArgumentParser(
        description="Whisper Real-time Speech-to-Text CLI",
        formatter_class=argparse.RawDescriptionHelpFormatter
    )
    
    parser.add_argument(
        '--configure',
        action='store_true',
        help='Configure microphone settings'
    )
    
    parser.add_argument(
        '--headless',
        action='store_true',
        help='Run in headless mode - single record/transcribe operation with output to stdout'
    )
    
    args = parser.parse_args()
    
    if args.configure:
        # Configuration mode
        cli = WhisperCLI(headless=False, force_configure=True)
        print("✅ Configuration complete!")
        return
    
    # Normal or headless mode
    cli = WhisperCLI(headless=args.headless)
    cli.run()

if __name__ == "__main__":
    main()