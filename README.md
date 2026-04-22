# remipn

[![CI](https://github.com/rhslack/remipn/actions/workflows/ci.yml/badge.svg)](https://github.com/rhslack/remipn/actions/workflows/ci.yml)

**Remipn** is a modern, lightweight, cross-platform VPN manager written in Rust. It offers an interactive Terminal User Interface (TUI) and a powerful Command Line Interface (CLI) to manage your VPN connections with ease.

## Features

- 🖥️ **Interactive TUI**: Manage your VPN profiles with an intuitive terminal interface based on `ratatui`.
- ⌨️ **CLI Support**: Quick commands to connect, disconnect, and check VPN status.
- 🔄 **Smart Connection Management**: Automatically handles switching between different VPNs, ensuring only one is active at a time.
- ⏳ **Real-time Feedback**: Connection status monitoring with an automatic retry mechanism and polling.
- 📁 **Profile Import**: Supports importing profiles from XML files, including automatic detection of Azure VPN Client profiles on macOS.
- 🚀 **Auto-Import**: Automatic scanning of default and system directories (`~/.config/remipn/imports/` and Azure VPN paths).
- 🔍 **Search and Filters**: Quickly find your profiles by name, category, or alias.
- 📂 **Cross-Platform**: Support for Windows (`rasdial`), Linux (`nmcli`), and macOS (`scutil`).
- ⌨️ **CLI Shorthands**: Quick command aliases (c, d, s, l) for power users.

## Installation

### Install latest prebuilt binary (recommended)

This installer detects your platform (`macOS`, `Linux`, or `Windows` via Git Bash/MSYS/Cygwin), downloads the latest compatible release asset, and installs it to `~/.local/bin`.

```bash
curl -fsSL https://raw.githubusercontent.com/rhslack/remipn/main/scripts/install.sh | bash
```

If `~/.local/bin` is not in your `PATH`, add it in your shell profile.

### Build from source

Make sure you have [Rust](https://www.rust-lang.org/) installed on your system.

```bash
# Clone the repository
git clone https://github.com/yourusername/remipn.git
cd remipn

# Build and install
cargo install --path .
```

## Usage

### TUI Interface

Simply run `remipn` without arguments to launch the interactive interface:

```bash
remipn
```

**Main Shortcuts:**
- `Enter`: Connect/Disconnect the selected profile
- `n`: Add a new profile
- `e`: Edit the selected profile
- `a`: Quick alias edit for the selected profile
- `x`: Delete the selected profile
- `i`: Import profiles from XML via file browser
- `I`: Manually trigger auto-import from standard locations (Azure VPN Client, etc.)
- `/`: Search through profiles
- `l`: Show/Hide logs
- `s`: Change sorting
- `q`: Quit

### CLI Interface

You can also use `remipn` directly from the command line with handy aliases:

```bash
# List all profiles (alias: l)
remipn list
remipn l

# Connect to a profile (alias: c)
remipn connect "ProfileName"
remipn c "alias"

# Disconnect (alias: d)
# Provide a name to disconnect a specific VPN, or no name to disconnect all
remipn disconnect
remipn d "ProfileName"

# Check status (alias: s)
remipn status
remipn s
```

## Configuration

Configurations are saved in `~/.config/remipn/config.toml`.  

**Profile Import Locations:**
- **Default**: `~/.config/remipn/imports/` (searched at startup or via `I`).
- **macOS Azure VPN**: `~/Library/Containers/com.microsoft.AzureVpnMac/Data/Library/Application Support/com.microsoft.AzureVpnMac` (automatically scanned).

Supported formats: `.xml`, `.ovpn`, `.azvpn`.

## License

This project is distributed under the MIT License. See the [LICENSE](LICENSE) file for details.

## Contributing

Contributions are welcome! Feel free to open issues or pull requests.
