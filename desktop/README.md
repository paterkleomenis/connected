# Connected Desktop

A native desktop application for the Connected ecosystem, enabling seamless file transfer and clipboard sync between devices.

## Features

- **Device Discovery**: Automatically discover other Connected devices on your local network via mDNS
- **File Transfer**: Send and receive files to/from any Connected device (Android, iOS, other desktops)
- **Clipboard Sync**: Real-time clipboard synchronization between devices
- **Apple-inspired UI**: Clean, modern interface with smooth animations

## Requirements

### Linux

```bash
# Debian/Ubuntu
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev xclip

# Arch Linux
sudo pacman -S webkit2gtk-4.1 gtk3 libappindicator-gtk3 xclip

# Fedora
sudo dnf install webkit2gtk4.1-devel gtk3-devel libappindicator-gtk3-devel xclip
```

### macOS

No additional dependencies required.

### Windows

No additional dependencies required.

## Building

From the workspace root (`connected/`):

```bash
# Debug build
cargo build -p connected-desktop-ui

# Release build (recommended)
cargo build -p connected-desktop-ui --release
```

## Running

```bash
# Run debug build
cargo run -p connected-desktop-ui

# Run release build
./target/release/connected-desktop-ui

# With debug logging
RUST_LOG=debug ./target/release/connected-desktop-ui
```

## Usage

### Scanning for Devices

1. Launch the application
2. Click "Scan Network" in the sidebar
3. Discovered devices will appear in the Devices tab

### Sending Files

1. Select a device from the list
2. Click the folder icon (📁) or go to the Transfers tab
3. Choose a file to send
4. Progress will be shown in the Transfers tab

### Clipboard Sync

1. Select a device from the list
2. Go to the Clipboard tab
3. Click "Sync with [Device Name]"
4. Your clipboard will automatically sync with the selected device

### Manual Clipboard Send

1. Go to the Clipboard tab
2. Enter or paste text in the text area
3. Select a device and click "Send"

## Ports

- **44444**: QUIC transport for file transfer and ping
- **44445**: Clipboard sync (QUIC port + 1)

Ensure these ports are open in your firewall for the application to work correctly.

## Troubleshooting

### No devices found

- Ensure all devices are on the same local network
- Check that mDNS/Bonjour is not blocked by your firewall
- On Linux, ensure avahi-daemon is running

### Clipboard sync not working

- Linux: Install `xclip` or `wl-copy` (Wayland)
- macOS: Should work out of the box with `pbcopy`/`pbpaste`

### Window not appearing

- Ensure you have a working display server (X11/Wayland)
- Check that WebKit2GTK is properly installed (Linux)

## Architecture

The desktop app uses:

- **Dioxus**: Rust-based UI framework with native desktop support
- **connected-core**: Shared Rust library for networking and protocol
- **WebKit2GTK**: Native web view for rendering (Linux)