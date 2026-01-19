# Connected

**Connected** is a high-speed, offline, cross-platform file transfer application built with **Rust** and **Jetpack Compose**. It aims to outperform proprietary ecosystems like AirDrop by utilizing cutting-edge technologies like BLE (Bluetooth Low Energy) for discovery and QUIC/Wi-Fi Direct for high-bandwidth data transfer.

Currently available for **Linux** and **Android**.

![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)
![Rust](https://img.shields.io/badge/rust-edition%202024-orange)
![Android](https://img.shields.io/badge/platform-Android%20%7C%20Linux-green)

## Features

-   **Cross-Platform**: Seamlessly transfer files between Android and Linux devices.
-   **Offline First**: No internet required. Uses Wi-Fi Direct and Local Network.
-   **High Speed**: Powered by the QUIC protocol over Wi-Fi 5/6.
-   **Zero Config**: Auto-discovery via mDNS and BLE.
-   **Secure**: End-to-end encryption for all transfers.
-   **Modern UI**: Beautiful, responsive interfaces using Jetpack Compose (Android) and Tailwind CSS (Linux Desktop).

## Screenshots

*(Placeholder for Screenshots - Add images of the Android and Linux UI here)*

## Installation

### Android

1.  Enable **Developer Options** on your Android device.
2.  Connect your device via USB.
3.  Navigate to the `android/` directory:
    ```bash
    cd android
    ```
4.  Build and install the app:
    ```bash
    ./gradlew installDebug
    ```

### Linux (Desktop)

**Requirements:**
-   Rust (stable)
-   `libasound2-dev`, `libudev-dev`, `libdbus-1-dev`, `pkg-config` (Ubuntu/Debian)

1.  Clone the repository:
    ```bash
    git clone https://github.com/paterkleomenis/connected.git
    cd connected
    ```
2.  Run the desktop application:
    ```bash
    cargo run -p connected-desktop
    ```

## Development

### Prerequisites

-   **Rust**: [Install Rust](https://rustup.rs/)
-   **Android Studio**: For Android development (SDK 34+ required).
-   **Cargo NDK**: Required for building the shared library for Android.
    ```bash
    cargo install cargo-ndk
    ```

### Project Structure

-   `core/`: Shared Rust logic (networking, discovery, encryption).
-   `desktop/`: Linux desktop application (Rust + Tauri-like WebView/UI logic).
-   `android/`: Native Android application (Kotlin + Jetpack Compose).
-   `ffi/`: UniFFI bindings to expose the Rust `core` to Kotlin.

## Contributing

Contributions are welcome! Please check out the issues tab or submit a pull request.

1.  Fork the repo.
2.  Create your feature branch (`git checkout -b feature/amazing-feature`).
3.  Commit your changes (`git commit -m 'Add some amazing feature'`).
4.  Push to the branch (`git push origin feature/amazing-feature`).
5.  Open a Pull Request.

## License

This project is licensed under either of

-   Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
-   MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
